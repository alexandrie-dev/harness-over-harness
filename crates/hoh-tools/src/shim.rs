//! Compatibility future alias retained for crates that imported it before the owners landed.

use std::{future::Future, pin::Pin};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
