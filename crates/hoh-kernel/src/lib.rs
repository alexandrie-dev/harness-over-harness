//! HoH's instance-owned kernel: fibers, reversible effects, services and typed events.
extern crate self as hoh_kernel;
pub mod config;
mod context;
mod effects;
mod events;
mod fiber;
mod logger;
pub use config::{Cfg, Config, Issue, NoConfig, PathSeg, ValidationError, Volatile};
pub use context::*;
pub use effects::*;
pub use events::*;
pub use fiber::*;
pub use hoh_kernel_macros::{Inject, plugin};
pub use logger::*;
use std::{
    any::Any,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard},
};
pub type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
type Task<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type Value = Arc<dyn Any + Send + Sync>;
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn read<T>(m: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|e| e.into_inner())
}
fn write<T>(m: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|e| e.into_inner())
}
#[derive(Debug, Clone, thiserror::Error)]
pub enum KernelError {
    #[error("inactive context")]
    Inactive,
    #[error("service \"{name}\" has been registered at <{owner}>")]
    DuplicateService { name: String, owner: String },
    #[error("service \"{name}\" is declared with another value type")]
    ServiceType { name: String },
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("one or more listeners of event \"{event}\" failed")]
    Listeners {
        event: &'static str,
        errors: Vec<Arc<Error>>,
    },
    #[error("{0}")]
    Failed(Arc<Error>),
    #[error("config type does not match the plugin's Config")]
    ConfigType,
    #[error("a fiber cannot wait for its own transition")]
    SelfWait,
    #[error("kernel has shut down")]
    Closed,
}
pub trait ServiceKey: Send + Sync + 'static {
    type Value: ?Sized + Send + Sync + 'static;
    const NAME: &'static str;
    fn view(_value: &Self::Value) -> Option<serde_json::Value> {
        None
    }
}
#[macro_export]
macro_rules! service {
    ($vis:vis $key:ident : $value:ty = $name:literal) => {
        $vis struct $key;
        impl $crate::ServiceKey for $key { type Value = $value; const NAME: &'static str = $name; }
    };
}
pub struct Service<K: ServiceKey> {
    value: Arc<K::Value>,
}
impl<K: ServiceKey> Clone for Service<K> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
        }
    }
}
impl<K: ServiceKey> std::ops::Deref for Service<K> {
    type Target = K::Value;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}
#[derive(Clone)]
pub struct Dep {
    pub name: &'static str,
    pub key: std::any::TypeId,
}
pub trait Inject: Sized + Send + 'static {
    fn deps() -> Vec<Dep>;
    #[doc(hidden)]
    fn resolve(view: &View) -> Self;
}
impl Inject for () {
    fn deps() -> Vec<Dep> {
        vec![]
    }
    fn resolve(_: &View) -> Self {}
}
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct View {
    pub(crate) values: indexmap::IndexMap<(Arc<str>, Realm), Arc<fiber::Provision>>,
}
impl View {
    #[doc(hidden)]
    pub fn service<K: ServiceKey>(&self) -> Service<K> {
        // The kernel checks every declared dependency before constructing a committed view.
        let value = self
            .values
            .values()
            .find(|p| p.name.as_ref() == K::NAME && p.key == std::any::TypeId::of::<K>())
            .and_then(|p| p.value.downcast_ref::<Arc<K::Value>>())
            .expect("committed dependency has its declared type");
        Service {
            value: value.clone(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Realm(u64);
impl Realm {
    pub const ROOT: Self = Self(0);
    pub fn new() -> Self {
        use std::hash::BuildHasher;
        Self(std::collections::hash_map::RandomState::new().hash_one("hoh realm") | 1)
    }
}
impl Default for Realm {
    fn default() -> Self {
        Self::new()
    }
}
