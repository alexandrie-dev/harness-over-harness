use crate::*;
use futures_util::FutureExt;
use std::{
    panic::AssertUnwindSafe,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;
#[cfg(test)]
#[path = "effects_review_tests.rs"]
mod review_tests;
type UndoAction = Box<dyn FnOnce() -> Task<Result<(), Error>> + Send>;

#[must_use = "setup эффекта обязан вернуть обратное действие"]
pub struct Undo {
    action: Option<UndoAction>,
    sync: Option<Box<dyn FnOnce() -> Result<(), Error> + Send>>,
    adopted: Option<Arc<Effect>>,
}

impl Undo {
    fn adopt(&self) {
        if let Some(effect) = &self.adopted {
            effect.adopted.store(true, Ordering::Release);
            effect.detach();
        }
    }
    pub fn none() -> Self {
        Self {
            action: None,
            sync: None,
            adopted: None,
        }
    }
    pub fn sync(f: impl FnOnce() + Send + 'static) -> Self {
        Self::try_sync(move || {
            f();
            Ok(())
        })
    }
    /// A synchronous cleanup that can fail without unwinding.
    pub fn try_sync(f: impl FnOnce() -> Result<(), Error> + Send + 'static) -> Self {
        Self {
            action: None,
            sync: Some(Box::new(f)),
            adopted: None,
        }
    }
    pub fn future<Fut>(f: impl FnOnce() -> Fut + Send + 'static) -> Self
    where
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        Self {
            action: Some(Box::new(move || Box::pin(f()))),
            sync: None,
            adopted: None,
        }
    }
    async fn run(mut self) -> Result<(), Error> {
        if let Some(sync) = self.sync.take() {
            sync()?;
        }
        if let Some(effect) = self.adopted.take() {
            effect.begin();
            effect.wait().await;
        }
        if let Some(action) = self.action.take() {
            action().await?;
        }
        Ok(())
    }
    fn run_sync(&mut self, propagate: bool) -> Result<bool, Error> {
        if let Some(effect) = &self.adopted {
            effect.begin_inner(propagate)?;
            if !effect.done.load(Ordering::Acquire) {
                return Ok(false);
            }
            self.adopted = None;
        }
        if self.action.is_some() {
            return Ok(false);
        }
        if let Some(sync) = self.sync.take() {
            sync()?;
        }
        Ok(true)
    }
}
impl From<Disposer> for Undo {
    fn from(disposer: Disposer) -> Self {
        Self {
            action: None,
            sync: None,
            adopted: Some(disposer.effect),
        }
    }
}
pub(crate) struct Effect {
    pub(crate) requested: AtomicBool,
    adopted: AtomicBool,
    running: AtomicBool,
    done: AtomicBool,
    state: Mutex<EffectData>,
    notify: Notify,
    label: Arc<str>,
    log: Logger,
    owner: std::sync::Weak<Activation>,
    ancestry: Vec<u32>,
}
struct EffectData {
    ready: bool,
    undos: Vec<Undo>,
    begin: Option<Box<dyn FnOnce() + Send>>,
}
impl Effect {
    pub(crate) fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
    pub(crate) fn add(&self, undo: Undo) {
        undo.adopt();
        lock(&self.state).undos.push(undo);
    }
    pub(crate) fn on_begin(&self, f: impl FnOnce() + Send + 'static) {
        lock(&self.state).begin = Some(Box::new(f));
    }
    fn new(
        label: impl Into<Arc<str>>,
        log: Logger,
        owner: std::sync::Weak<Activation>,
        ancestry: Vec<u32>,
    ) -> Arc<Self> {
        Arc::new(Self {
            requested: AtomicBool::new(false),
            adopted: AtomicBool::new(false),
            running: AtomicBool::new(false),
            done: AtomicBool::new(false),
            state: Mutex::new(EffectData {
                ready: false,
                undos: vec![],
                begin: None,
            }),
            notify: Notify::new(),
            label: label.into(),
            log,
            owner,
            ancestry,
        })
    }
    pub(crate) fn finish_setup(&self) {
        lock(&self.state).ready = true;
        self.notify.notify_waiters();
    }
    fn detach(&self) {
        if let Some(owner) = self.owner.upgrade() {
            lock(&owner.effects)
                .effects
                .retain(|e| !std::ptr::eq(e.as_ref(), self));
        }
    }
    fn complete(&self) {
        self.detach();
        self.done.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }
    pub(crate) fn retire_child(self: &Arc<Self>) {
        self.requested.store(true, Ordering::Release);
        if !self.running.swap(true, Ordering::AcqRel) {
            let undos = std::mem::take(&mut lock(&self.state).undos);
            drop(undos);
            self.complete();
        } else {
            self.detach();
        }
    }
    fn synchronous(&self, undos: &mut Vec<Undo>, propagate: bool) -> Result<(), Error> {
        while let Some(mut undo) = undos.pop() {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                fiber::sync_scope(self.ancestry.clone(), || undo.run_sync(propagate))
            }))
            .unwrap_or_else(|payload| {
                let text = payload
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("cleanup panicked");
                Err(Error::from(text))
            });
            match result {
                Ok(true) => {}
                Ok(false) => {
                    undos.push(undo);
                    break;
                }
                Err(error) => {
                    if propagate {
                        return Err(error);
                    }
                    self.log
                        .error(format_args!("effect \"{}\": {error}", self.label));
                }
            }
        }
        Ok(())
    }
    fn late(self: &Arc<Self>, undo: Undo) {
        let mut pending = vec![undo];
        let _ = self.synchronous(&mut pending, false);
        if pending.is_empty() {
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            self.log.error(format_args!(
                "effect \"{}\": asynchronous disposal requires a tokio runtime",
                self.label
            ));
            return;
        }
        let effect = self.clone();
        fiber::spawn_for(self.ancestry.clone(), async move {
            effect.cleanup(pending).await;
        });
    }
    async fn cleanup(&self, undos: Vec<Undo>) {
        for undo in undos.into_iter().rev() {
            match AssertUnwindSafe(undo.run()).catch_unwind().await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => self
                    .log
                    .error(format_args!("effect \"{}\": {e}", self.label)),
                Err(_) => self
                    .log
                    .error(format_args!("effect \"{}\": cleanup panicked", self.label)),
            }
        }
    }
    pub(crate) fn begin(self: &Arc<Self>) {
        let _ = self.begin_inner(false);
    }
    fn rollback(self: &Arc<Self>) -> Result<(), KernelError> {
        self.begin_inner(true)
            .map_err(|error| KernelError::Failed(Arc::new(error)))
    }
    fn begin_inner(self: &Arc<Self>, propagate: bool) -> Result<(), Error> {
        self.requested.store(true, Ordering::Release);
        if self.running.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let begin = lock(&self.state).begin.take();
        if let Some(begin) = begin {
            begin();
        }
        let pending = {
            let mut state = lock(&self.state);
            state.ready.then(|| std::mem::take(&mut state.undos))
        };
        let pending = match pending {
            Some(mut undos) => {
                if let Err(error) = self.synchronous(&mut undos, propagate) {
                    drop(undos);
                    self.complete();
                    return Err(error);
                }
                Some(undos)
            }
            None => None,
        };
        if pending.as_ref().is_some_and(Vec::is_empty) {
            self.complete();
            return Ok(());
        }
        if tokio::runtime::Handle::try_current().is_err() {
            self.log.error(format_args!(
                "effect \"{}\": asynchronous disposal requires a tokio runtime",
                self.label
            ));
            let undos = std::mem::take(&mut lock(&self.state).undos);
            drop(undos);
            self.complete();
            return Ok(());
        }
        let effect = self.clone();
        fiber::spawn_for(self.ancestry.clone(), async move {
            loop {
                let notified = effect.notify.notified();
                if lock(&effect.state).ready {
                    break;
                }
                notified.await;
            }
            let undos = pending.unwrap_or_else(|| std::mem::take(&mut lock(&effect.state).undos));
            effect.cleanup(undos).await;
            effect.complete();
        });
        Ok(())
    }
    pub(crate) async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.done.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}
impl Drop for Effect {
    fn drop(&mut self) {
        if !self.done.load(Ordering::Acquire) && !lock(&self.state).undos.is_empty() {
            self.log.error(format_args!(
                "effect \"{}\" of <{}> dropped without disposal",
                self.label,
                self.log.fiber_name()
            ));
        }
    }
}
#[derive(Clone)]
pub struct Undos {
    effect: Arc<Effect>,
}
impl Undos {
    pub fn push(&self, undo: impl Into<Undo>) -> Result<(), KernelError> {
        let undo = undo.into();
        undo.adopt();
        let mut state = lock(&self.effect.state);
        let inactive = self.effect.requested.load(Ordering::Acquire);
        if inactive && state.ready {
            drop(state);
            self.effect.late(undo);
            return Err(KernelError::Inactive);
        }
        state.undos.push(undo);
        if inactive {
            Err(KernelError::Inactive)
        } else {
            Ok(())
        }
    }
}
pub struct Disposer {
    pub(crate) effect: Arc<Effect>,
}
impl Disposer {
    pub fn dispose(self) -> impl Future<Output = ()> + Send + 'static {
        self.effect.begin();
        async move { self.effect.wait().await }
    }
}
pub(crate) struct Activation {
    pub(crate) effects: Mutex<ActivationData>,
    pub(crate) token: tokio_util::sync::CancellationToken,
    pub(crate) view: RwLock<View>,
    pub(crate) committed: RwLock<std::sync::Weak<RwLock<View>>>,
    pub(crate) cfg: Arc<RwLock<Value>>,
}
pub(crate) struct ActivationData {
    pub(crate) closed: bool,
    effects: Vec<Arc<Effect>>,
}
impl Activation {
    pub(crate) fn abandon(&self) {
        let effects = std::mem::take(&mut lock(&self.effects).effects);
        drop(effects);
    }
    pub(crate) fn new(view: View) -> Arc<Self> {
        Arc::new(Self {
            effects: Mutex::new(ActivationData {
                closed: false,
                effects: vec![],
            }),
            token: tokio_util::sync::CancellationToken::new(),
            view: RwLock::new(view),
            committed: RwLock::default(),
            cfg: Arc::new(RwLock::new(Arc::new(()))),
        })
    }
    pub(crate) fn close(&self) {
        let mut state = lock(&self.effects);
        state.closed = true;
        self.token.cancel();
        for effect in &state.effects {
            effect.requested.store(true, Ordering::Release);
        }
        drop(state);
        let old = std::mem::replace(&mut *write(&self.view), View::empty());
        drop(old);
    }
    pub(crate) async fn unload(&self) {
        let effects = std::mem::take(&mut lock(&self.effects).effects);
        let effects = effects
            .into_iter()
            .rev()
            .filter(|e| !e.adopted.load(Ordering::Acquire))
            .collect::<Vec<_>>();
        for effect in &effects {
            effect.begin();
        }
        for effect in effects {
            effect.wait().await;
        }
    }
}
impl Context {
    pub(crate) fn reserve_effect(
        &self,
        label: impl Into<Arc<str>>,
    ) -> Result<Disposer, KernelError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(KernelError::Closed);
        }
        let effect = Effect::new(
            label,
            self.logger(),
            Arc::downgrade(&self.activation),
            self.data.ancestry.clone(),
        );
        {
            let mut state = lock(&self.activation.effects);
            if state.closed {
                return Err(KernelError::Inactive);
            }
            state.effects.push(effect.clone());
        }
        Ok(Disposer { effect })
    }
    pub fn effect(
        &self,
        label: impl Into<Arc<str>>,
        setup: impl FnOnce() -> Undo,
    ) -> Result<Disposer, KernelError> {
        let disposer = self.reserve_effect(label)?;
        let result = std::panic::catch_unwind(AssertUnwindSafe(setup));
        if let Ok(undo) = result {
            disposer.effect.add(undo);
            disposer.effect.finish_setup();
            Ok(disposer)
        } else {
            disposer.effect.finish_setup();
            disposer.effect.begin();
            Err(KernelError::Failed(Arc::new(
                "effect setup panicked".into(),
            )))
        }
    }
    pub fn try_effect<E: From<KernelError>>(
        &self,
        label: impl Into<Arc<str>>,
        setup: impl FnOnce(&Undos) -> Result<(), E>,
    ) -> Result<Disposer, E> {
        let disposer = self.reserve_effect(label)?;
        let undos = Undos {
            effect: disposer.effect.clone(),
        };
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| setup(&undos)));
        disposer.effect.finish_setup();
        match result {
            Ok(Ok(())) => Ok(disposer),
            Ok(Err(error)) => {
                disposer.effect.rollback()?;
                Err(error)
            }
            Err(_) => {
                disposer.effect.rollback()?;
                Err(KernelError::Failed(Arc::new("effect setup panicked".into())).into())
            }
        }
    }
    pub async fn effect_async<E, Fut>(
        &self,
        label: impl Into<Arc<str>>,
        setup: impl FnOnce(Undos) -> Fut,
    ) -> Result<Disposer, E>
    where
        E: From<KernelError>,
        Fut: Future<Output = Result<(), E>> + Send,
    {
        let disposer = self.reserve_effect(label)?;
        let undos = Undos {
            effect: disposer.effect.clone(),
        };
        struct Finish(Arc<Effect>);
        impl Drop for Finish {
            fn drop(&mut self) {
                self.0.finish_setup();
            }
        }
        let finish = Finish(disposer.effect.clone());
        let result = AssertUnwindSafe(async { setup(undos).await })
            .catch_unwind()
            .await;
        drop(finish);
        match result {
            Ok(Ok(())) => Ok(disposer),
            Ok(Err(error)) => {
                disposer.effect.begin();
                disposer.effect.wait().await;
                Err(error)
            }
            Err(_) => {
                disposer.effect.begin();
                disposer.effect.wait().await;
                Err(KernelError::Failed(Arc::new("effect setup panicked".into())).into())
            }
        }
    }
    pub fn spawn<Fut>(
        &self,
        label: impl Into<Arc<str>>,
        task: impl FnOnce(tokio_util::sync::CancellationToken) -> Fut + Send + 'static,
    ) -> Result<Disposer, KernelError>
    where
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        self.effect(label, || {
            let token = self.token().child_token();
            let cancel = token.clone();
            let log = self.logger();
            let task = fiber::spawn_for(self.data.ancestry.clone(), async move {
                if let Err(e) = task(token).await {
                    log.error(e);
                }
            });
            Undo::future(move || async move {
                cancel.cancel();
                task.await.map_err(|e| Box::new(e) as Error)?;
                Ok(())
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn dropped_undo_reports_loss_without_executing_cleanup() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let called = Arc::new(AtomicBool::new(false));
        let c = called.clone();
        let disposer = ctx
            .effect("lost", move || {
                Undo::sync(move || {
                    c.store(true, Ordering::Release);
                })
            })
            .unwrap();
        ctx.activation.abandon();
        drop(disposer);
        assert!(!called.load(Ordering::Acquire));
        assert_eq!(
            ctx.logger().buffer().last().unwrap().message,
            "effect \"lost\" of <root> dropped without disposal"
        );
        kernel.shutdown().await;
    }
}
