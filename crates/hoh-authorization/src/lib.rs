//! Authorization flows that obtain credentials through human interaction.

use std::{
    collections::HashMap,
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};

use futures_util::future::{BoxFuture, Either, select};
use hoh_credentials::{CredentialKey, CredentialRecord, Credentials, CredentialsRecordUpdated};
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{
    Contained, Context, Disposer, Inject, ListenerFailure, NoConfig, Plugin, Service, Undo,
};
use hoh_timeout::{AbortController, AbortSignal};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Constructor namespace for stable authorization failures.
pub struct AuthorizationError;

impl AuthorizationError {
    pub const NAME: &'static str = "AuthorizationError";

    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> HarnessError {
        HarnessError::named(Self::NAME, message, code)
    }
}

/// Constructor namespace for a human declining an interaction prompt.
pub struct AuthorizationDeclinedError;

impl AuthorizationDeclinedError {
    pub const NAME: &'static str = "AuthorizationDeclinedError";
    pub const CODE: &'static str = "DECLINED";
    pub const DEFAULT_MESSAGE: &'static str = "the authorization prompt was declined";

    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> HarnessError {
        Self::with_message(Self::DEFAULT_MESSAGE)
    }

    pub fn with_message(message: impl Into<String>) -> HarnessError {
        HarnessError::named(Self::NAME, message, Self::CODE)
    }

    pub fn is(error: &(dyn Error + 'static)) -> bool {
        HarnessError::of(error).is_some_and(|error| error.is_named(Self::NAME))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationMethod {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationNotice {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationPromptOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AuthorizationPrompt {
    Text {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        #[serde(skip)]
        signal: Option<AbortSignal>,
    },
    Secret {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        #[serde(skip)]
        signal: Option<AbortSignal>,
    },
    Select {
        message: String,
        options: Vec<AuthorizationPromptOption>,
        #[serde(skip)]
        signal: Option<AbortSignal>,
    },
}

impl AuthorizationPrompt {
    pub fn signal(&self) -> Option<&AbortSignal> {
        match self {
            Self::Text { signal, .. }
            | Self::Secret { signal, .. }
            | Self::Select { signal, .. } => signal.as_ref(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthorizationStatus {
    Authorized,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthorizationSettlement {
    Authorized,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationOutcome {
    pub status: AuthorizationStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationEntry {
    pub key: CredentialKey,
    pub label: String,
    pub methods: Vec<AuthorizationMethod>,
    pub in_flight: bool,
}

pub type AuthorizationFuture<T> = BoxFuture<'static, Result<T, BoxError>>;
pub type AuthorizationRun =
    Arc<dyn Fn(AuthorizationSession) -> AuthorizationFuture<()> + Send + Sync>;

/// A plugin-owned way to obtain one credential record.
#[derive(Clone)]
pub struct AuthorizationFlow {
    key: CredentialKey,
    label: String,
    methods: Vec<AuthorizationMethod>,
    run: AuthorizationRun,
}

impl fmt::Debug for AuthorizationFlow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizationFlow")
            .field("key", &self.key)
            .field("label", &self.label)
            .field("methods", &self.methods)
            .finish_non_exhaustive()
    }
}

impl AuthorizationFlow {
    pub fn new<F, Fut>(
        key: CredentialKey,
        label: impl Into<String>,
        first_method: AuthorizationMethod,
        other_methods: impl IntoIterator<Item = AuthorizationMethod>,
        run: F,
    ) -> Self
    where
        F: Fn(AuthorizationSession) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), BoxError>> + Send + 'static,
    {
        let mut methods = vec![first_method];
        methods.extend(other_methods);
        Self {
            key,
            label: label.into(),
            methods,
            run: Arc::new(move |session| Box::pin(run(session))),
        }
    }

    pub fn key(&self) -> &CredentialKey {
        &self.key
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn methods(&self) -> &[AuthorizationMethod] {
        &self.methods
    }
}

pub trait AuthorizationInteraction: Send + Sync {
    fn notify(&self, notice: AuthorizationNotice) -> Result<(), BoxError>;
    fn prompt(&self, prompt: AuthorizationPrompt) -> AuthorizationFuture<String>;
}

#[derive(Clone)]
pub struct AuthorizationRequest {
    pub key: CredentialKey,
    pub method: Option<String>,
    pub interaction: Arc<dyn AuthorizationInteraction>,
    pub signal: Option<AbortSignal>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationSettled {
    pub key: CredentialKey,
    pub settlement: AuthorizationSettlement,
}

hoh_kernel::event!(AuthorizationSettled = "authorization/settled", Contained);

struct Observed {
    declined: AtomicBool,
    committed: AtomicBool,
}

#[derive(Clone)]
pub struct AuthorizationSession {
    method: String,
    signal: AbortSignal,
    key: CredentialKey,
    attempt_id: u64,
    inner: Arc<Inner>,
    interaction: Arc<dyn AuthorizationInteraction>,
    observed: Arc<Observed>,
}

impl AuthorizationSession {
    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn signal(&self) -> &AbortSignal {
        &self.signal
    }

    pub async fn commit(&self, record: CredentialRecord) -> Result<(), BoxError> {
        self.signal.throw_if_aborted()?;
        {
            let mut state = lock(&self.inner.state);
            let Some(attempt) = state.running.get_mut(&self.key) else {
                return Err(AuthorizationError::new(
                    "authorization attempt is no longer active",
                    "CANCELLED",
                )
                .into());
            };
            if !attempt.admit_commit(self.attempt_id) {
                return Err(AuthorizationError::new(
                    "authorization attempt is no longer active",
                    "CANCELLED",
                )
                .into());
            }
        }
        self.inner
            .credentials
            .modify_record(
                &self.key,
                Box::new(move |_| Box::pin(async move { Ok(Some(record)) })),
            )
            .await?;
        Ok(())
    }

    pub fn notify(&self, notice: AuthorizationNotice) {
        if let Err(error) = self.interaction.notify(notice) {
            let logger = self.inner.ctx.logger();
            logger.warn("authorization: the interaction surface failed to render a notice");
            logger.warn(ErrorChain(error.as_ref()));
        }
    }

    pub async fn prompt(&self, prompt: AuthorizationPrompt) -> Result<String, BoxError> {
        match self.interaction.prompt(prompt).await {
            Ok(answer) => Ok(answer),
            Err(error) => {
                if AuthorizationDeclinedError::is(error.as_ref()) {
                    self.observed.declined.store(true, Ordering::Release);
                }
                Err(error)
            }
        }
    }
}

struct InFlight {
    id: u64,
    controller: Arc<AbortController>,
    committing: bool,
    withdrawn: bool,
    completion_admitted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionAdmission {
    Completed,
    Withdrawn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunCompletion {
    Cancelled,
    Settled,
}

impl InFlight {
    fn admit_commit(&mut self, attempt_id: u64) -> bool {
        if self.id != attempt_id || self.withdrawn || self.completion_admitted {
            return false;
        }
        self.committing = true;
        true
    }

    fn admit_withdrawal(&mut self, attempt_id: Option<u64>) -> Option<Arc<AbortController>> {
        if self.committing
            || self.withdrawn
            || self.completion_admitted
            || attempt_id.is_some_and(|attempt_id| self.id != attempt_id)
        {
            return None;
        }
        self.withdrawn = true;
        Some(self.controller.clone())
    }

    fn admit_completion(&mut self, attempt_id: u64) -> CompletionAdmission {
        if self.id != attempt_id || self.withdrawn {
            return CompletionAdmission::Withdrawn;
        }
        // This is the completion side of the same cancellation race decided
        // by admit_withdrawal. Once the run finishes first, a later cancel
        // cannot retroactively change its Failed/Authorized result.
        self.completion_admitted = true;
        CompletionAdmission::Completed
    }
}

fn admit_run_completion(
    attempt: Option<&mut InFlight>,
    attempt_id: u64,
    signal_aborted: bool,
    declined: bool,
    result: &Result<(), BoxError>,
) -> RunCompletion {
    let admission = attempt.map_or(CompletionAdmission::Withdrawn, |attempt| {
        attempt.admit_completion(attempt_id)
    });
    if admission == CompletionAdmission::Withdrawn
        || (result.is_err() && (signal_aborted || declined))
    {
        RunCompletion::Cancelled
    } else {
        RunCompletion::Settled
    }
}

#[derive(Default)]
struct State {
    flows: IndexMap<CredentialKey, AuthorizationFlow>,
    running: HashMap<CredentialKey, InFlight>,
    next_attempt_id: u64,
}

struct Inner {
    ctx: Context,
    credentials: Service<Credentials>,
    state: Mutex<State>,
}

#[derive(Clone)]
pub struct AuthorizationService(Arc<Inner>);

hoh_kernel::service!(pub Authorization: AuthorizationService = "authorization");

impl AuthorizationService {
    pub fn new(ctx: Context, credentials: Service<Credentials>) -> Self {
        Self(Arc::new(Inner {
            ctx,
            credentials,
            state: Mutex::new(State::default()),
        }))
    }

    pub fn register_flow(
        &self,
        owner: &Context,
        flow: AuthorizationFlow,
    ) -> Result<Disposer, BoxError> {
        let key = flow.key.clone();
        let service = self.clone();
        let cleanup_key = key.clone();
        owner.try_effect::<BoxError>("authorization.registerFlow()", move |undos| {
            {
                let mut state = lock(&service.0.state);
                if state.flows.contains_key(&key) {
                    return Err(AuthorizationError::new(
                        format!("an authorization flow for \"{key}\" is already registered"),
                        "DUPLICATE_FLOW",
                    )
                    .into());
                }
                state.flows.insert(key, flow);
            }
            let cleanup = service.clone();
            undos.push(Undo::sync(move || cleanup.unregister(&cleanup_key)))?;
            Ok(())
        })
    }

    fn unregister(&self, key: &CredentialKey) {
        lock(&self.0.state).flows.shift_remove(key);
        self.cancel(key);
    }

    pub fn list(&self) -> Vec<AuthorizationEntry> {
        let state = lock(&self.0.state);
        state
            .flows
            .values()
            .map(|flow| Self::entry(&state, flow))
            .collect()
    }

    pub fn describe(&self, key: &CredentialKey) -> Option<AuthorizationEntry> {
        let state = lock(&self.0.state);
        state.flows.get(key).map(|flow| Self::entry(&state, flow))
    }

    fn entry(state: &State, flow: &AuthorizationFlow) -> AuthorizationEntry {
        AuthorizationEntry {
            key: flow.key.clone(),
            label: flow.label.clone(),
            methods: flow.methods.clone(),
            in_flight: state.running.contains_key(&flow.key),
        }
    }

    pub fn cancel(&self, key: &CredentialKey) {
        self.withdraw(key, None, None);
    }

    fn withdraw(
        &self,
        key: &CredentialKey,
        attempt_id: Option<u64>,
        reason: Option<hoh_error::SharedError>,
    ) {
        let controller = {
            let mut state = lock(&self.0.state);
            state
                .running
                .get_mut(key)
                .and_then(|attempt| attempt.admit_withdrawal(attempt_id))
        };
        if let Some(controller) = controller {
            if let Some(reason) = reason {
                controller.abort_with(reason);
            } else {
                controller.abort();
            }
        }
    }

    pub async fn begin(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationOutcome, BoxError> {
        let (send, receive) = tokio::sync::oneshot::channel();
        let service = self.clone();
        let key = request.key.clone();
        self.0
            .ctx
            .spawn("authorization.begin()", move |lifetime| async move {
                let operation = service.run_begin(request);
                tokio::pin!(operation);
                let result = tokio::select! {
                    biased;
                    result = &mut operation => result,
                    () = lifetime.cancelled() => {
                        service.cancel(&key);
                        operation.await
                    }
                };
                let _ = send.send(result);
                Ok(())
            })?;
        receive.await.map_err(|_| -> BoxError {
            "authorization: attempt task ended without an outcome".into()
        })?
    }

    async fn run_begin(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationOutcome, BoxError> {
        let key = request.key.clone();
        let (flow, method, attempt_id, signal) = {
            let mut state = lock(&self.0.state);
            let flow = state.flows.get(&key).cloned().ok_or_else(|| {
                Box::new(AuthorizationError::new(
                    format!("no authorization flow is registered for \"{key}\""),
                    "NO_FLOW",
                )) as BoxError
            })?;
            let method = request
                .method
                .clone()
                .unwrap_or_else(|| flow.methods[0].id.clone());
            if !flow.methods.iter().any(|candidate| candidate.id == method) {
                return Err(AuthorizationError::new(
                    format!("authorization flow for \"{key}\" offers no method \"{method}\""),
                    "UNKNOWN_METHOD",
                )
                .into());
            }
            if state.running.contains_key(&key) {
                return Err(AuthorizationError::new(
                    format!("an authorization attempt for \"{key}\" is already running"),
                    "ALREADY_IN_FLIGHT",
                )
                .into());
            }
            if request.signal.as_ref().is_some_and(AbortSignal::aborted) {
                return Ok(AuthorizationOutcome {
                    status: AuthorizationStatus::Cancelled,
                });
            }
            state.next_attempt_id = state.next_attempt_id.wrapping_add(1);
            let attempt_id = state.next_attempt_id;
            let controller = Arc::new(AbortController::new());
            let signal = controller.signal();
            state.running.insert(
                key.clone(),
                InFlight {
                    id: attempt_id,
                    controller,
                    committing: false,
                    withdrawn: false,
                    completion_admitted: false,
                },
            );
            (flow, method, attempt_id, signal)
        };

        let attempt = self.attempt(flow, method, attempt_id, signal, request.interaction);
        tokio::pin!(attempt);
        let result = if let Some(caller_signal) = request.signal {
            tokio::select! {
                result = &mut attempt => result,
                reason = caller_signal.cancelled() => {
                    self.withdraw(&key, Some(attempt_id), Some(reason));
                    attempt.await
                }
            }
        } else {
            attempt.await
        };

        {
            let mut state = lock(&self.0.state);
            if state
                .running
                .get(&key)
                .is_some_and(|attempt| attempt.id == attempt_id)
            {
                state.running.remove(&key);
            }
        }
        let settlement = match &result {
            Ok(AuthorizationOutcome {
                status: AuthorizationStatus::Authorized,
            }) => AuthorizationSettlement::Authorized,
            Ok(AuthorizationOutcome {
                status: AuthorizationStatus::Cancelled,
            }) => AuthorizationSettlement::Cancelled,
            Err(_) => AuthorizationSettlement::Failed,
        };
        self.settle(key, settlement);
        result
    }

    async fn attempt(
        &self,
        flow: AuthorizationFlow,
        method: String,
        attempt_id: u64,
        signal: AbortSignal,
        interaction: Arc<dyn AuthorizationInteraction>,
    ) -> Result<AuthorizationOutcome, BoxError> {
        let observed = Arc::new(Observed {
            declined: AtomicBool::new(false),
            committed: AtomicBool::new(false),
        });
        let watched_key = flow.key.clone();
        let watcher_observed = observed.clone();
        let watcher = self
            .0
            .ctx
            .on_contained::<CredentialsRecordUpdated>(move |event| {
                if event.key == watched_key {
                    watcher_observed.committed.store(true, Ordering::Release);
                }
                Ok(())
            })?;
        let session = AuthorizationSession {
            method,
            signal: signal.clone(),
            key: flow.key.clone(),
            attempt_id,
            inner: self.0.clone(),
            interaction,
            observed: observed.clone(),
        };
        let running = (flow.run)(session);
        let withdrawn: Pin<Box<dyn Future<Output = hoh_error::SharedError> + Send>> =
            Box::pin(signal.cancelled());
        let result = match select(running, withdrawn).await {
            Either::Left((result, _)) => {
                let completion = {
                    let mut state = lock(&self.0.state);
                    admit_run_completion(
                        state.running.get_mut(&flow.key),
                        attempt_id,
                        signal.aborted(),
                        observed.declined.load(Ordering::Acquire),
                        &result,
                    )
                };
                match completion {
                    RunCompletion::Cancelled => {
                        watcher.dispose().await;
                        return Ok(AuthorizationOutcome {
                            status: AuthorizationStatus::Cancelled,
                        });
                    }
                    RunCompletion::Settled => result,
                }
            }
            Either::Right((_, running)) => {
                let logger = self.0.ctx.logger();
                let _ =
                    self.0
                        .ctx
                        .spawn("authorization withdrawn flow", move |lifetime| async move {
                            tokio::select! {
                                result = running => {
                                    if result.is_err() {
                                        logger.debug(
                                            "authorization: withdrawn flow failed after the fact",
                                        );
                                    }
                                }
                                () = lifetime.cancelled() => {}
                            }
                            Ok(())
                        });
                watcher.dispose().await;
                return Ok(AuthorizationOutcome {
                    status: AuthorizationStatus::Cancelled,
                });
            }
        };
        watcher.dispose().await;
        result?;

        if !observed.committed.load(Ordering::Acquire) {
            return Err(AuthorizationError::new(
                format!(
                    "authorization flow for \"{}\" resolved without committing a credential record in this attempt",
                    flow.key
                ),
                "NOT_COMMITTED",
            )
            .into());
        }
        if !self
            .0
            .credentials
            .describe_record(&flow.key)
            .await?
            .configured
        {
            return Err(AuthorizationError::new(
                format!(
                    "authorization flow for \"{}\" deleted its credential record instead of committing one",
                    flow.key
                ),
                "NOT_COMMITTED",
            )
            .into());
        }
        Ok(AuthorizationOutcome {
            status: AuthorizationStatus::Authorized,
        })
    }

    fn settle(&self, key: CredentialKey, settlement: AuthorizationSettlement) {
        let logger = self.0.ctx.logger();
        let subject = key.clone();
        self.0
            .ctx
            .root()
            .dispatch(AuthorizationSettled { key, settlement })
            .invoke(move |failure| {
                let error = match failure {
                    ListenerFailure::Threw(error) | ListenerFailure::Rejected(error) => error,
                };
                logger.warn(format_args!(
                    "authorization: an authorization/settled listener for \"{subject}\" failed"
                ));
                logger.warn(ErrorChain(error.as_ref()));
            });
    }
}

struct ErrorChain<'a>(&'a (dyn Error + 'static));

impl fmt::Display for ErrorChain<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)?;
        let mut cause = self.0.source();
        while let Some(error) = cause {
            write!(formatter, ": {error}")?;
            cause = error.source();
        }
        Ok(())
    }
}

#[derive(Inject)]
pub struct AuthorizationDeps {
    credentials: Service<Credentials>,
}

pub struct AuthorizationPlugin;

impl Plugin for AuthorizationPlugin {
    const NAME: &'static str = "AuthorizationService";
    type Config = NoConfig;
    type Deps = AuthorizationDeps;

    async fn apply(
        ctx: Context,
        deps: AuthorizationDeps,
        _config: hoh_kernel::Cfg<NoConfig>,
    ) -> Result<(), BoxError> {
        let service = AuthorizationService::new(ctx.clone(), deps.credentials);
        ctx.provide::<Authorization>(Arc::new(service)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_flight(id: u64) -> InFlight {
        InFlight {
            id,
            controller: Arc::new(AbortController::new()),
            committing: false,
            withdrawn: false,
            completion_admitted: false,
        }
    }

    #[test]
    fn withdrawal_and_commit_admission_are_a_single_ordered_decision() {
        let mut withdrawn_first = in_flight(7);
        assert!(withdrawn_first.admit_withdrawal(Some(7)).is_some());
        assert!(!withdrawn_first.admit_commit(7));
        assert!(withdrawn_first.admit_withdrawal(Some(7)).is_none());

        let mut commit_first = in_flight(8);
        assert!(commit_first.admit_commit(8));
        assert!(commit_first.admit_withdrawal(Some(8)).is_none());

        let mut current = in_flight(9);
        assert!(current.admit_withdrawal(Some(8)).is_none());
        assert!(current.admit_commit(9));
    }

    #[test]
    fn withdrawal_and_run_completion_are_a_single_ordered_decision() {
        let mut withdrawn_first = in_flight(10);
        let controller = withdrawn_first.admit_withdrawal(Some(10)).unwrap();
        // Reproduce N1 exactly: the withdrawal decision is committed, but the
        // controller has not been aborted outside the mutex yet.
        assert!(!controller.signal().aborted());
        let cancelled_error: BoxError =
            AuthorizationError::new("authorization attempt is no longer active", "CANCELLED")
                .into();
        let rejected_commit = Err(cancelled_error);
        assert_eq!(
            admit_run_completion(
                Some(&mut withdrawn_first),
                10,
                false,
                false,
                &rejected_commit,
            ),
            RunCompletion::Cancelled
        );

        let mut completed_first = in_flight(11);
        assert_eq!(
            admit_run_completion(Some(&mut completed_first), 11, false, false, &Ok(()),),
            RunCompletion::Settled
        );
        // N2: a flow which won the race keeps its own result; cancellation can
        // no longer turn NOT_COMMITTED/Failed into Cancelled after the fact.
        assert!(completed_first.admit_withdrawal(Some(11)).is_none());
        assert!(!completed_first.admit_commit(11));

        let mut committed_first = in_flight(12);
        assert!(committed_first.admit_commit(12));
        assert_eq!(
            committed_first.admit_completion(12),
            CompletionAdmission::Completed
        );
        assert!(committed_first.admit_withdrawal(Some(12)).is_none());
    }
}
