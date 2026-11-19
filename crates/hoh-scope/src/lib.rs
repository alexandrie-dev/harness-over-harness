//! Port of @deepseek-ai/dsh-scope (packages/core/scope).
//! Scope identities, routed event carriers, and scope-owned registration tables.
//!
//! `create_scope` immediately returns a real child context, ready for registrations
//! before its backing fiber activates. See `PORT_NOTES.md` for the consumer API.

pub mod store;
pub use store::{AnonymousEntries, NamedEntries, ScopeLayer, ScopedLayers};

use hoh_error::BoxError;
use hoh_kernel::{Context, Disposer, Fiber, KernelError};
use std::{
    collections::HashSet,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

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

impl ScopeKey {
    /// A non-owning handle to this key's identity; it does not keep the key alive.
    pub fn downgrade(&self) -> WeakScopeKey {
        WeakScopeKey {
            id: self.0.id,
            data: Arc::downgrade(&self.0),
        }
    }
}

/// A weak scope key: the source's `WeakMap` key. Equality and hashing follow the identity of the
/// original key and stay stable after it dies. A weak key keeps the key's allocation reserved, so
/// a freshly minted key never compares equal to a dead one.
#[derive(Clone)]
pub struct WeakScopeKey {
    id: u64,
    data: Weak<KeyData>,
}

impl WeakScopeKey {
    /// The key while some strong handle to it is alive.
    pub fn upgrade(&self) -> Option<ScopeKey> {
        self.data.upgrade().map(ScopeKey)
    }
}
impl PartialEq for WeakScopeKey {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.data, &other.data)
    }
}
impl Eq for WeakScopeKey {}
impl Hash for WeakScopeKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}
impl fmt::Debug for WeakScopeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("WeakScopeKey").field(&self.id).finish()
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
    fiber: Fiber,
    // Keep the exact parent-owned record alive so the accessor remains usable after teardown.
    _raw_dispose: Disposer,
}
impl Scope {
    /// The exact child effect, suitable for ordered adoption with `Undos::push`.
    /// Repeated reads return handles of the same kernel record.
    pub fn raw_dispose(&self) -> Disposer {
        // `_raw_dispose` retains this child's parent-owned record for the lifetime of Scope.
        self.fiber
            .disposer()
            .expect("scope retains its raw disposer")
    }
    /// Repeated calls join the same kernel teardown, including raw-disposer-first calls.
    pub fn dispose(&self) -> impl Future<Output = ()> + Send + 'static {
        self.fiber.dispose()
    }
}

struct ScopeTag(ScopeKey);

/// Mint a synchronously usable scope; parent placement is bound before creation.
pub fn create_scope(
    ctx: &Context,
    key: ScopeKey,
    options: CreateScopeOptions,
) -> Result<Scope, BoxError> {
    if let Some(parent) = options.parent {
        bind_scope_parent(&key, parent)?;
    }
    let scoped = ctx.child("scope")?.with_tag(ScopeTag(key));
    let fiber = scoped.fiber();
    let raw = fiber.disposer().ok_or(KernelError::Inactive)?;
    Ok(Scope {
        ctx: scoped,
        fiber,
        _raw_dispose: raw,
    })
}

pub fn scope_of(ctx: &Context) -> Option<ScopeKey> {
    ctx.tag::<ScopeTag>().map(|tag| tag.0.clone())
}

type Filter = Arc<dyn Fn(&Context) -> bool + Send + Sync>;

/// The optional Cordis `Context.filter` property, expressed as a Rust method.
/// Every subject explicitly implements its filter, including service isolation.
pub trait ScopeFilter: Send + Sync + 'static {
    fn matches_filter(&self, listener: &Context) -> bool;
}
impl ScopeFilter for Context {
    fn matches_filter(&self, listener: &Context) -> bool {
        Context::matches_filter(self, listener)
    }
}
impl ScopeFilter for () {
    fn matches_filter(&self, _: &Context) -> bool {
        true
    }
}

/// An opaque, typed routing carrier. It contains no subject properties.
///
/// ```compile_fail
/// struct Subject { value: u32 }
/// impl hoh_scope::ScopeFilter for Subject {
///     fn matches_filter(&self, _: &hoh_kernel::Context) -> bool { true }
/// }
/// let carrier = hoh_scope::scope_target(std::sync::Arc::new(Subject { value: 1 }), None);
/// carrier.value; // The subject is not exposed through the carrier.
/// ```
pub struct Scoped<T: ?Sized> {
    key: Option<ScopeKey>,
    base_filter: Filter,
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
    pub fn admits(&self, ctx: &Context) -> bool {
        if !(self.base_filter)(ctx) {
            return false;
        }
        match scope_of(ctx) {
            None => true,
            Some(tag) => scope_chain_of(self.key.as_ref()).contains(&tag),
        }
    }
    /// Derive a dispatch context. All five kernel dispatch modes use this filter;
    /// `Listen::global()` retains the kernel's bypass semantics.
    /// Filtering belongs to the carrier's base, independently of the emitter's filter.
    pub fn context(&self, ctx: &Context) -> Context {
        let carrier = Scoped::<()> {
            key: self.key.clone(),
            base_filter: self.base_filter.clone(),
            subject: PhantomData,
        };
        ctx.with_filter(move |listener| carrier.admits(listener))
    }
}

impl<T: ?Sized + 'static> ScopeFilter for Scoped<T> {
    fn matches_filter(&self, listener: &Context) -> bool {
        self.admits(listener)
    }
}

pub fn scope_target<T: ScopeFilter + ?Sized>(base: Arc<T>, key: Option<ScopeKey>) -> Scoped<T> {
    Scoped {
        key,
        base_filter: Arc::new(move |listener| base.matches_filter(listener)),
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
