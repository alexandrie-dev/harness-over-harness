//! Port of @deepseek-ai/dsh-scope (packages/core/scope).
//! Scope identities, routed event carriers, and scope-owned registration tables.
//!
//! `create_scope` awaits a real child activation; registrations belong to that child.
//! See `PORT_NOTES.md` for kernel limitations compared with Cordis.

pub mod store;
pub use store::{AnonymousEntries, NamedEntries, ScopeLayer, ScopedLayers};

use hoh_kernel::{Context, Error, Fiber, KernelError, Undo, plugin};
use std::{
    collections::HashSet,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::oneshot;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// An opaque, identity-compared scope key. Clone preserves identity.
#[derive(Clone)]
pub struct ScopeKey(Arc<KeyData>);

struct KeyData {
    id: u64,
    parent: Mutex<Option<ScopeKey>>,
}

impl ScopeKey {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(Arc::new(KeyData {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            parent: Mutex::new(None),
        }))
    }
}

impl Default for ScopeKey {
    fn default() -> Self {
        Self::new()
    }
}
impl PartialEq for ScopeKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ScopeKey {}
impl Hash for ScopeKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state);
    }
}
impl fmt::Debug for ScopeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ScopeKey").field(&self.0.id).finish()
    }
}
impl Drop for KeyData {
    fn drop(&mut self) {
        // Parent chains have no depth bound; release uniquely owned links iteratively.
        let mut parent = lock(&self.parent).take();
        while let Some(key) = parent {
            match Arc::try_unwrap(key.0) {
                Ok(data) => parent = lock(&data.parent).take(),
                Err(_) => break,
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScopeError {
    #[error("dsh-scope: scope parent link would form a cycle")]
    Cycle,
    #[error(
        "dsh-scope: scope key is already bound to a parent; re-linking requires the binding returned by the original bind"
    )]
    AlreadyBound,
}

/// The only handle allowed to move a previously bound key's parent.
pub struct ScopeParentBinding {
    key: ScopeKey,
}
impl ScopeParentBinding {
    /// The holder must uphold the source's blank-session recompose contract.
    pub fn rebind(&self, parent: ScopeKey) -> Result<(), ScopeError> {
        link_scope_parent(&self.key, parent, false)
    }
}

fn link_scope_parent(key: &ScopeKey, parent: ScopeKey, initial: bool) -> Result<(), ScopeError> {
    loop {
        let chain = scope_chain_of(Some(&parent));
        let mut nodes = chain.clone();
        nodes.push(key.clone());
        nodes.sort_unstable_by_key(|node| node.0.id);
        nodes.dedup();
        // A deterministic lock order makes writes across independently minted keys atomic.
        let mut guards: Vec<_> = nodes.iter().map(|node| lock(&node.0.parent)).collect();
        let position = |node: &ScopeKey| {
            nodes
                .binary_search_by_key(&node.0.id, |n| n.0.id)
                // Every queried node was inserted above and ids are unique for the process lifetime.
                .expect("scope node belongs to the locked chain")
        };
        if initial && guards[position(key)].is_some() {
            return Err(ScopeError::AlreadyBound);
        }
        let stable = chain
            .iter()
            .enumerate()
            .all(|(index, node)| guards[position(node)].as_ref() == chain.get(index + 1));
        if !stable {
            continue;
        }
        if chain.contains(key) {
            return Err(ScopeError::Cycle);
        }
        *guards[position(key)] = Some(parent);
        return Ok(());
    }
}

pub fn bind_scope_parent(
    key: &ScopeKey,
    parent: ScopeKey,
) -> Result<ScopeParentBinding, ScopeError> {
    link_scope_parent(key, parent, true)?;
    Ok(ScopeParentBinding { key: key.clone() })
}

pub fn scope_parent_of(key: &ScopeKey) -> Option<ScopeKey> {
    lock(&key.0.parent).clone()
}

/// Keys nearest first: `[key, parent, grandparent, …]`.
pub fn scope_chain_of(key: Option<&ScopeKey>) -> Vec<ScopeKey> {
    loop {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = key.cloned();
        while let Some(key) = cursor {
            cursor = scope_parent_of(&key);
            // A concurrent rebind can make a reader revisit a node in an old snapshot.
            if !seen.insert(key.0.id) {
                break;
            }
            chain.push(key);
        }
        let mut sorted = chain.clone();
        sorted.sort_unstable_by_key(|node| node.0.id);
        let guards: Vec<_> = sorted.iter().map(|node| lock(&node.0.parent)).collect();
        let stable = chain.iter().enumerate().all(|(index, node)| {
            let position = sorted
                .binary_search_by_key(&node.0.id, |key| key.0.id)
                // Every chain node appears exactly once in the sorted snapshot.
                .expect("scope node belongs to the read snapshot");
            guards[position].as_ref() == chain.get(index + 1)
        });
        if stable {
            return chain;
        }
    }
}

#[derive(Default)]
pub struct CreateScopeOptions {
    pub parent: Option<ScopeKey>,
}

/// A scope's context and its raw child-fiber disposal boundary.
pub struct Scope {
    pub ctx: Context,
    pub raw_dispose: ScopeDisposer,
}
impl Scope {
    /// Repeated calls join the same kernel teardown, including raw-disposer-first calls.
    pub fn dispose(&self) -> impl Future<Output = ()> + Send + 'static {
        self.raw_dispose.dispose()
    }
}

/// A reusable raw fiber disposer, also accepted by an ordered `Undos` composite.
#[derive(Clone)]
pub struct ScopeDisposer {
    fiber: Fiber,
}
impl ScopeDisposer {
    pub fn dispose(&self) -> impl Future<Output = ()> + Send + 'static {
        self.fiber.dispose()
    }
}
impl From<ScopeDisposer> for Undo {
    fn from(disposer: ScopeDisposer) -> Self {
        Undo::future(move || async move {
            disposer.dispose().await;
            Ok(())
        })
    }
}

struct Handoff(Mutex<Option<oneshot::Sender<Context>>>);
struct ScopeTag(ScopeKey);

#[plugin(ScopePlugin, name = "scope")]
async fn apply_scope(ctx: Context) -> Result<(), Error> {
    if let Some(handoff) = ctx.tag::<Handoff>()
        && let Some(sender) = lock(&handoff.0).take()
    {
        let _ = sender.send(ctx);
    }
    Ok(())
}

/// Mint a scope. The current kernel exposes the child context only from its apply.
pub async fn create_scope(
    ctx: &Context,
    key: ScopeKey,
    options: CreateScopeOptions,
) -> Result<Scope, Error> {
    if let Some(parent) = options.parent {
        bind_scope_parent(&key, parent)?;
    }
    let (sender, receiver) = oneshot::channel();
    let tagged = ctx
        .with_tag(ScopeTag(key))
        .with_tag(Handoff(Mutex::new(Some(sender))));
    let fiber = tagged.plugin::<ScopePlugin>(serde_json::Value::Null)?;
    let token = ctx.token();
    let scoped = tokio::select! {
        biased;
        result = receiver => result.map_err(|_| KernelError::Inactive),
        _ = token.cancelled() => Err(KernelError::Inactive),
    };
    match scoped {
        Ok(ctx) => Ok(Scope {
            ctx,
            raw_dispose: ScopeDisposer { fiber },
        }),
        Err(error) => {
            fiber.dispose().await;
            Err(error.into())
        }
    }
}

pub fn scope_of(ctx: &Context) -> Option<ScopeKey> {
    ctx.tag::<ScopeTag>().map(|tag| tag.0.clone())
}

type Filter = Arc<dyn Fn(&Context) -> bool + Send + Sync>;

/// An opaque, typed routing carrier. It contains no subject properties.
///
/// ```compile_fail
/// let carrier = hoh_scope::scope_target(&String::from("subject"), None);
/// carrier.len(); // The subject is not exposed through the carrier.
/// ```
pub struct Scoped<T: ?Sized> {
    key: Option<ScopeKey>,
    base_filter: Option<Filter>,
    subject: PhantomData<fn(&T)>,
}
impl<T: ?Sized> Clone for Scoped<T> {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            base_filter: self.base_filter.clone(),
            subject: PhantomData,
        }
    }
}
impl<T: ?Sized> Scoped<T> {
    /// Explicitly supply the base filter; capture its receiver when it needs one.
    pub fn with_base_filter(
        mut self,
        filter: impl Fn(&Context) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.base_filter = Some(Arc::new(filter));
        self
    }
    pub fn admits(&self, ctx: &Context) -> bool {
        if self.base_filter.as_ref().is_some_and(|filter| !filter(ctx)) {
            return false;
        }
        match scope_of(ctx) {
            None => true,
            Some(tag) => scope_chain_of(self.key.as_ref()).contains(&tag),
        }
    }
    /// Derive a dispatch context. All five kernel dispatch modes use this filter;
    /// `Listen::global()` retains the kernel's bypass semantics.
    /// Existing filters must be supplied through `with_base_filter` (see port notes).
    pub fn context(&self, ctx: &Context) -> Context {
        let carrier = Scoped::<()> {
            key: self.key.clone(),
            base_filter: self.base_filter.clone(),
            subject: PhantomData,
        };
        ctx.with_filter(move |listener| carrier.admits(listener))
    }
}

pub fn scope_target<T: ?Sized>(_base: &T, key: Option<ScopeKey>) -> Scoped<T> {
    Scoped {
        key,
        base_filter: None,
        subject: PhantomData,
    }
}

/// Carrier membership is statically enforced in Rust, including unkeyed carriers.
///
/// ```compile_fail
/// hoh_scope::is_scope_carrier(&String::from("not a carrier"));
/// ```
pub fn is_scope_carrier<T: ?Sized>(_value: &Scoped<T>) -> bool {
    true
}
pub fn carrier_key_of<T: ?Sized>(value: &Scoped<T>) -> Option<ScopeKey> {
    value.key.clone()
}
