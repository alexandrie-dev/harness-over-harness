//! Registration table for HoH window methods owned by sibling plugins.
use crate::{error::error, lock};
use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_kernel::{Context, Disposer, Undo};
use serde_json::Value;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};

type MethodHandler =
    Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value, BoxError>> + Send + Sync>;

struct MethodEntry {
    token: u64,
    handler: MethodHandler,
}

#[derive(Default)]
struct State {
    next_token: u64,
    methods: HashMap<String, MethodEntry>,
}

/// GLUE-SHIM(4): decides one privileged call (`hoh_protocol::PRIVILEGED_METHODS`) in host mode.
/// `Ok(())` means the user allowed exactly this call; any error refuses it. The approval part
/// mounts the real one (human text, `approval/request`, mac); until then the server fails closed.
/// Handlers of privileged methods must not ask the user again: the server asks before them.
pub trait PrivilegedApprover: Send + Sync + 'static {
    fn approve(&self, method: &str, params: &Value) -> BoxFuture<'static, Result<(), BoxError>>;
}
hoh_kernel::service!(pub HohPrivilegedApprover: dyn PrivilegedApprover = "hohPrivilegedApprover");

/// Shared dispatch table for the `hoh/*` methods implemented by composed plugins.
#[derive(Clone, Default)]
pub struct HohRpcMethodRegistry {
    state: Arc<Mutex<State>>,
}

hoh_kernel::service!(pub HohRpcMethods: HohRpcMethodRegistry = "hohRpcMethods");

impl HohRpcMethodRegistry {
    /// Register one protocol-declared `hoh/*` method for the lifetime of `ctx`.
    pub fn register<F, Fut>(
        &self,
        ctx: &Context,
        method: impl Into<String>,
        handler: F,
    ) -> Result<Disposer, BoxError>
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, BoxError>> + Send + 'static,
    {
        let method = method.into();
        if !method.starts_with("hoh/") || !hoh_protocol::REQUEST_METHODS.contains(&method.as_str())
        {
            return Err(error(format!(
                "cannot register undeclared HoH RPC method: {method}"
            )));
        }
        let handler: MethodHandler = Arc::new(move |params| Box::pin(handler(params)));
        let registry = self.clone();
        let label = format!("hohRpcMethods.register({method})");
        ctx.try_effect(label, move |undos| -> Result<(), BoxError> {
            let token = {
                let mut state = lock(&registry.state);
                if state.methods.contains_key(&method) {
                    return Err(error(format!(
                        "HoH RPC method is already registered: {method}"
                    )));
                }
                state.next_token += 1;
                let token = state.next_token;
                state.methods.insert(
                    method.clone(),
                    MethodEntry {
                        token,
                        handler: handler.clone(),
                    },
                );
                token
            };
            let cleanup = registry.clone();
            undos.push(Undo::sync(move || {
                let mut state = lock(&cleanup.state);
                if state
                    .methods
                    .get(&method)
                    .is_some_and(|entry| entry.token == token)
                {
                    state.methods.remove(&method);
                }
            }))?;
            Ok(())
        })
    }

    pub(crate) fn is_registered(&self, method: &str) -> bool {
        lock(&self.state).methods.contains_key(method)
    }

    pub(crate) fn invoke(
        &self,
        method: &str,
        params: Value,
    ) -> Option<BoxFuture<'static, Result<Value, BoxError>>> {
        let handler = lock(&self.state)
            .methods
            .get(method)
            .map(|entry| entry.handler.clone())?;
        Some(handler(params))
    }
}
