//! Credential-scoped model-listing cache with singleflight background refresh.

use crate::listing::ModelListing;
use crate::listing_config::ListingSettings;
use futures_util::future::{BoxFuture, FutureExt, Shared};
use hoh_error::BoxError;
use hoh_timeout::{AbortController, AbortSignal};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub type SettingsCallback = Arc<dyn Fn() -> Option<ListingSettings> + Send + Sync>;
pub type CredentialCallback =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<Option<String>, BoxError>> + Send + Sync>;
pub type LoadCallback = Arc<
    dyn Fn(String, String, AbortSignal) -> BoxFuture<'static, Result<ModelListing, BoxError>>
        + Send
        + Sync,
>;

#[derive(Clone)]
pub struct ListingCacheDependencies {
    pub settings: SettingsCallback,
    pub credential: CredentialCallback,
    pub load: LoadCallback,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub info: Arc<dyn Fn(String) + Send + Sync>,
    pub warn: Arc<dyn Fn(String) + Send + Sync>,
    pub changed: Arc<dyn Fn() + Send + Sync>,
}

impl ListingCacheDependencies {
    pub fn with_system_clock(mut self) -> Self {
        self.now = Arc::new(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |value| value.as_millis() as u64)
        });
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListingView {
    pub authenticated: bool,
    pub listing: Option<ModelListing>,
}

type LoadTask = Shared<BoxFuture<'static, ()>>;

#[derive(Default)]
struct State {
    scope: Option<String>,
    listing: Option<ModelListing>,
    settled_at: Option<u64>,
    inflight: Option<LoadTask>,
    request: Option<Arc<AbortController>>,
    timer: Option<Arc<AbortController>>,
    pending: Vec<LoadTask>,
    failing: bool,
    closed: bool,
    generation: u64,
}

#[derive(Clone)]
pub struct ModelListingCache {
    dependencies: ListingCacheDependencies,
    state: Arc<Mutex<State>>,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl ModelListingCache {
    pub fn new(dependencies: ListingCacheDependencies) -> Self {
        Self {
            dependencies,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn current(&self) -> ListingView {
        let state = lock(&self.state);
        ListingView {
            authenticated: state.scope.is_some(),
            listing: state.listing.clone(),
        }
    }

    pub async fn view(
        &self,
        settings: ListingSettings,
        signal: Option<&AbortSignal>,
    ) -> Result<ListingView, BoxError> {
        let credential = (self.dependencies.credential)(settings.url.clone()).await?;
        if let Some(signal) = signal {
            signal.throw_if_aborted()?;
        }
        if lock(&self.state).closed {
            return Ok(self.current());
        }
        let scope = credential.as_ref().map(|credential| {
            let digest = Sha256::digest(credential.as_bytes());
            let digest = digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            format!("{}\n{digest}", settings.url)
        });
        if lock(&self.state).scope != scope {
            self.rescope(scope);
        }
        let Some(credential) = credential else {
            return Ok(self.current());
        };
        let first = lock(&self.state).settled_at.is_none();
        let stale = {
            let state = lock(&self.state);
            state.settled_at.is_none_or(|at| {
                (self.dependencies.now)().saturating_sub(at) >= settings.max_age_ms
            })
        };
        if first {
            let task = self.start(settings, credential);
            wait_for_task(task, signal).await?;
        } else if stale {
            std::mem::drop(self.start(settings, credential));
        }
        Ok(self.current())
    }

    fn start(&self, settings: ListingSettings, credential: String) -> LoadTask {
        let request = Arc::new(AbortController::new());
        let mut state = lock(&self.state);
        if let Some(inflight) = &state.inflight {
            return inflight.clone();
        }
        state.generation = state.generation.wrapping_add(1);
        state.request = Some(request.clone());
        let generation = state.generation;
        let cache = self.clone();
        let task = async move {
            let limit = hoh_timeout::deadline(
                Some(&request.signal()),
                settings.timeout_ms,
                crate::listing::LISTING_TIMEOUT,
            );
            let outcome = match limit {
                Ok(limit) => {
                    let outcome = (cache.dependencies.load)(
                        settings.url.clone(),
                        credential,
                        limit.signal.clone(),
                    )
                    .await;
                    drop(limit);
                    outcome
                }
                Err(error) => Err(Box::new(error) as BoxError),
            };
            cache.settle(generation, &settings, outcome);
        }
        .boxed()
        .shared();
        if state.generation == generation && !state.closed {
            state.inflight = Some(task.clone());
            state.pending.push(task.clone());
        }
        drop(state);
        self.drive_tracked(task.clone());
        task
    }

    fn drive_tracked(&self, task: LoadTask) {
        let state = self.state.clone();
        tokio::spawn(async move {
            task.await;
            lock(&state)
                .pending
                .retain(|pending| pending.peek().is_none());
        });
    }

    fn settle(
        &self,
        generation: u64,
        settings: &ListingSettings,
        outcome: Result<ModelListing, BoxError>,
    ) {
        let mut changed = false;
        {
            let mut state = lock(&self.state);
            if state.generation != generation || state.closed {
                return;
            }
            state.inflight = None;
            state.request = None;
            state.settled_at = Some((self.dependencies.now)());
            match outcome {
                Ok(listing) => {
                    changed = state.listing.as_ref() != Some(&listing);
                    (self.dependencies.info)(format!(
                        "model listing from {} loaded {} model(s), skipped {} row(s)",
                        settings.url,
                        listing.entries.len(),
                        listing.skipped
                    ));
                    state.listing = Some(listing);
                    state.failing = false;
                }
                Err(error) => {
                    if !state.failing {
                        let fallback = if state.listing.is_none() {
                            "using the bundled catalog"
                        } else {
                            "keeping the previous catalog"
                        };
                        (self.dependencies.warn)(format!(
                            "model listing from {} failed ({error}); {fallback}",
                            settings.url
                        ));
                    }
                    state.failing = true;
                }
            }
        }
        if changed {
            self.notify_changed();
        }
        self.schedule(settings.clone(), generation);
    }

    fn schedule(&self, settings: ListingSettings, generation: u64) {
        if settings.max_age_ms == 0 {
            return;
        }
        let timer = Arc::new(AbortController::new());
        let timer_signal = timer.signal();
        let cache = self.clone();
        let task = async move {
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(
                    settings.max_age_ms.clamp(1, hoh_timeout::MAX_TIMER_DELAY_MS),
                )) => {}
                _ = timer_signal.cancelled() => return,
            }
            {
                let state = lock(&cache.state);
                if state.closed || state.generation != generation {
                    return;
                }
            }
            let Some(latest) = (cache.dependencies.settings)() else {
                let was_authenticated = cache.current().authenticated;
                cache.rescope(None);
                if was_authenticated {
                    cache.notify_changed();
                }
                return;
            };
            if let Err(error) = cache.view(latest.clone(), None).await {
                let mut state = lock(&cache.state);
                if !state.closed && !state.failing {
                    (cache.dependencies.warn)(format!(
                        "model listing refresh failed ({error}); keeping the previous catalog; check listing configuration and credentials"
                    ));
                    state.failing = true;
                    state.settled_at = Some((cache.dependencies.now)());
                }
            }
            let generation = {
                let state = lock(&cache.state);
                (!state.closed && state.inflight.is_none()).then_some(state.generation)
            };
            if let Some(generation) = generation {
                cache.schedule(latest, generation);
            }
        }
        .boxed()
        .shared();
        {
            let mut state = lock(&self.state);
            if state.closed || state.generation != generation {
                return;
            }
            if let Some(previous) = state.timer.replace(timer) {
                previous.abort();
            }
            state.pending.push(task.clone());
        }
        self.drive_tracked(task);
    }

    fn notify_changed(&self) {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.dependencies.changed)()
        }))
        .is_err()
        {
            (self.dependencies.warn)(
                "model listing change notification failed; reload the model catalog".into(),
            );
        }
    }

    fn rescope(&self, scope: Option<String>) {
        let mut state = lock(&self.state);
        if let Some(request) = state.request.take() {
            request.abort();
        }
        if let Some(timer) = state.timer.take() {
            timer.abort();
        }
        state.scope = scope;
        state.listing = None;
        state.settled_at = None;
        state.inflight = None;
        state.failing = false;
        state.generation = state.generation.wrapping_add(1);
    }

    pub fn invalidate(&self) {
        let scope = lock(&self.state).scope.clone();
        self.rescope(scope);
    }

    pub async fn dispose(&self) {
        let pending = {
            let mut state = lock(&self.state);
            state.closed = true;
            state.generation = state.generation.wrapping_add(1);
            if let Some(request) = state.request.take() {
                request.abort();
            }
            if let Some(timer) = state.timer.take() {
                timer.abort();
            }
            state.inflight = None;
            std::mem::take(&mut state.pending)
        };
        futures_util::future::join_all(pending).await;
    }
}

async fn wait_for_task(task: LoadTask, signal: Option<&AbortSignal>) -> Result<(), BoxError> {
    if let Some(signal) = signal {
        tokio::select! {
            () = task => Ok(()),
            reason = signal.cancelled() => Err(Box::new(reason)),
        }
    } else {
        task.await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use tokio::sync::Notify;

    fn listing(skipped: usize) -> ModelListing {
        ModelListing {
            entries: vec![],
            skipped,
        }
    }

    fn settings(max_age_ms: u64) -> ListingSettings {
        ListingSettings {
            url: "https://example.test/models".into(),
            timeout_ms: 1_000.0,
            max_age_ms,
        }
    }

    fn dependencies(
        now: Arc<AtomicU64>,
        load: LoadCallback,
        changed: Arc<AtomicUsize>,
    ) -> ListingCacheDependencies {
        ListingCacheDependencies {
            settings: Arc::new(|| None),
            credential: Arc::new(|_| async { Ok(Some("secret".into())) }.boxed()),
            load,
            now: Arc::new(move || now.load(Ordering::SeqCst)),
            info: Arc::new(|_| {}),
            warn: Arc::new(|_| {}),
            changed: Arc::new(move || {
                changed.fetch_add(1, Ordering::SeqCst);
            }),
        }
    }

    async fn wait_for_count(counter: &AtomicUsize, expected: usize) {
        for _ in 0..1_000 {
            if counter.load(Ordering::SeqCst) >= expected {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("load count did not reach {expected}");
    }

    async fn wait_for_pending(cache: &ModelListingCache, expected: usize) {
        for _ in 0..1_000 {
            if lock(&cache.state).pending.len() == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!(
            "pending task count did not reach {expected}; remained {}",
            lock(&cache.state).pending.len()
        );
    }

    #[tokio::test]
    async fn completed_refresh_tasks_do_not_accumulate() {
        let calls = Arc::new(AtomicUsize::new(0));
        let load: LoadCallback = {
            let calls = calls.clone();
            Arc::new(move |_, _, _| {
                let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                async move { Ok(listing(call)) }.boxed()
            })
        };
        let cache = ModelListingCache::new(dependencies(
            Arc::new(AtomicU64::new(0)),
            load,
            Arc::new(AtomicUsize::new(0)),
        ));
        for expected in 1..=3 {
            cache.view(settings(0), None).await.unwrap();
            wait_for_count(&calls, expected).await;
            wait_for_pending(&cache, 0).await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        cache.dispose().await;
    }

    #[tokio::test]
    async fn first_views_share_one_load_and_independent_waits() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Notify::new());
        let load: LoadCallback = {
            let calls = calls.clone();
            let gate = gate.clone();
            Arc::new(move |_, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                let gate = gate.clone();
                async move {
                    gate.notified().await;
                    Ok(listing(1))
                }
                .boxed()
            })
        };
        let cache = ModelListingCache::new(dependencies(
            Arc::new(AtomicU64::new(0)),
            load,
            Arc::new(AtomicUsize::new(0)),
        ));
        let left = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.view(settings(100), None).await })
        };
        let controller = AbortController::new();
        let signal = controller.signal();
        let abandoned = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.view(settings(100), Some(&signal)).await })
        };
        wait_for_count(&calls, 1).await;
        controller.abort();
        assert!(abandoned.await.unwrap().is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        gate.notify_one();
        assert_eq!(left.await.unwrap().unwrap().listing, Some(listing(1)));
    }

    #[tokio::test]
    async fn stale_view_returns_immediately_and_refreshes_in_background() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Notify::new());
        let load: LoadCallback = {
            let calls = calls.clone();
            let gate = gate.clone();
            Arc::new(move |_, _, _| {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                let gate = gate.clone();
                async move {
                    if call > 0 {
                        gate.notified().await;
                    }
                    Ok(listing(call + 1))
                }
                .boxed()
            })
        };
        let now = Arc::new(AtomicU64::new(0));
        let changed = Arc::new(AtomicUsize::new(0));
        let cache = ModelListingCache::new(dependencies(now.clone(), load, changed.clone()));
        assert_eq!(
            cache.view(settings(1_000_000), None).await.unwrap().listing,
            Some(listing(1))
        );
        now.store(1_000_000, Ordering::SeqCst);
        assert_eq!(
            cache.view(settings(1_000_000), None).await.unwrap().listing,
            Some(listing(1))
        );
        wait_for_count(&calls, 2).await;
        gate.notify_one();
        for _ in 0..1_000 {
            if cache.current().listing == Some(listing(2)) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(cache.current().listing, Some(listing(2)));
        assert_eq!(changed.load(Ordering::SeqCst), 2);
        cache.dispose().await;
    }

    #[tokio::test]
    async fn automatic_refresh_discovers_replacement_after_sign_out() {
        let credential = Arc::new(Mutex::new(Some("first".to_owned())));
        let calls = Arc::new(AtomicUsize::new(0));
        let load: LoadCallback = {
            let calls = calls.clone();
            Arc::new(move |_, _, _| {
                let value = calls.fetch_add(1, Ordering::SeqCst) + 1;
                async move { Ok(listing(value)) }.boxed()
            })
        };
        let mut dependencies = dependencies(
            Arc::new(AtomicU64::new(0)),
            load,
            Arc::new(AtomicUsize::new(0)),
        );
        dependencies.settings = Arc::new(|| Some(settings(1)));
        dependencies.credential = {
            let credential = credential.clone();
            Arc::new(move |_| {
                let value = lock(&credential).clone();
                async move { Ok(value) }.boxed()
            })
        };
        let cache = ModelListingCache::new(dependencies);
        assert_eq!(
            cache.view(settings(1), None).await.unwrap().listing,
            Some(listing(1))
        );

        *lock(&credential) = None;
        for _ in 0..100 {
            if !cache.current().authenticated {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(!cache.current().authenticated);

        *lock(&credential) = Some("replacement".into());
        wait_for_count(&calls, 2).await;
        for _ in 0..100 {
            if cache.current().listing == Some(listing(2)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(cache.current().listing, Some(listing(2)));
        cache.dispose().await;
    }

    #[tokio::test]
    async fn automatic_refresh_retries_credential_failures() {
        let credential_calls = Arc::new(AtomicUsize::new(0));
        let load_calls = Arc::new(AtomicUsize::new(0));
        let now = Arc::new(AtomicU64::new(0));
        let load: LoadCallback = {
            let load_calls = load_calls.clone();
            Arc::new(move |_, _, _| {
                let value = load_calls.fetch_add(1, Ordering::SeqCst) + 1;
                async move { Ok(listing(value)) }.boxed()
            })
        };
        let mut dependencies = dependencies(now.clone(), load, Arc::new(AtomicUsize::new(0)));
        dependencies.settings = Arc::new(|| Some(settings(1)));
        dependencies.credential = {
            let credential_calls = credential_calls.clone();
            let now = now.clone();
            Arc::new(move |_| {
                let call = credential_calls.fetch_add(1, Ordering::SeqCst);
                now.store(call as u64 * 10, Ordering::SeqCst);
                async move {
                    if matches!(call, 1 | 2) {
                        Err(Box::new(std::io::Error::other("credentials offline")) as BoxError)
                    } else {
                        Ok(Some("token".into()))
                    }
                }
                .boxed()
            })
        };
        let cache = ModelListingCache::new(dependencies);
        assert_eq!(
            cache.view(settings(1), None).await.unwrap().listing,
            Some(listing(1))
        );
        for _ in 0..100 {
            if credential_calls.load(Ordering::SeqCst) >= 4
                && load_calls.load(Ordering::SeqCst) >= 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(credential_calls.load(Ordering::SeqCst) >= 4);
        assert!(load_calls.load(Ordering::SeqCst) >= 2);
        for _ in 0..100 {
            if cache.current().listing == Some(listing(2)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(cache.current().listing, Some(listing(2)));
        cache.dispose().await;
    }

    #[tokio::test]
    async fn disposal_waits_for_admitted_automatic_credential_lookup() {
        let credential_calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Notify::new());
        let load: LoadCallback = Arc::new(|_, _, _| async { Ok(listing(1)) }.boxed());
        let mut dependencies = dependencies(
            Arc::new(AtomicU64::new(0)),
            load,
            Arc::new(AtomicUsize::new(0)),
        );
        dependencies.settings = Arc::new(|| Some(settings(1)));
        dependencies.credential = {
            let credential_calls = credential_calls.clone();
            let gate = gate.clone();
            Arc::new(move |_| {
                let call = credential_calls.fetch_add(1, Ordering::SeqCst);
                let gate = gate.clone();
                async move {
                    if call > 0 {
                        gate.notified().await;
                    }
                    Ok(Some("token".into()))
                }
                .boxed()
            })
        };
        let cache = ModelListingCache::new(dependencies);
        cache.view(settings(1), None).await.unwrap();
        for _ in 0..100 {
            if credential_calls.load(Ordering::SeqCst) >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(credential_calls.load(Ordering::SeqCst) >= 2);

        let disposal = tokio::spawn({
            let cache = cache.clone();
            async move { cache.dispose().await }
        });
        tokio::task::yield_now().await;
        assert!(!disposal.is_finished());
        gate.notify_one();
        disposal.await.unwrap();
    }
}
