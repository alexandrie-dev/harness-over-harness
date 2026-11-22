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
type PromptListener = Arc<dyn Fn(&str, &str, &Value) + Send + Sync>;

struct MethodEntry {
    token: u64,
    handler: MethodHandler,
}

#[derive(Default)]
struct State {
    next_token: u64,
    methods: HashMap<String, MethodEntry>,
    prompt_listener: Option<(u64, PromptListener)>,
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

    /// GLUE (HOH-40): hear every `session/prompt` — the session id, the id of the message it
    /// queues, the whole `params` — once its session and message exist and before the message
    /// can start a turn, for the lifetime of `ctx`. The fields the server does not read itself
    /// (the window's `effort`) reach the plugin that serves them, tied to their own message. One
    /// listener.
    pub fn on_prompt<F>(&self, ctx: &Context, listener: F) -> Result<Disposer, BoxError>
    where
        F: Fn(&str, &str, &Value) + Send + Sync + 'static,
    {
        let listener: PromptListener = Arc::new(listener);
        let registry = self.clone();
        ctx.try_effect(
            "hohRpcMethods.onPrompt",
            move |undos| -> Result<(), BoxError> {
                let token = {
                    let mut state = lock(&registry.state);
                    if state.prompt_listener.is_some() {
                        return Err(error("a session/prompt listener is already registered"));
                    }
                    state.next_token += 1;
                    let token = state.next_token;
                    state.prompt_listener = Some((token, listener.clone()));
                    token
                };
                let cleanup = registry.clone();
                undos.push(Undo::sync(move || {
                    let mut state = lock(&cleanup.state);
                    if state
                        .prompt_listener
                        .as_ref()
                        .is_some_and(|(owner, _)| *owner == token)
                    {
                        state.prompt_listener = None;
                    }
                }))?;
                Ok(())
            },
        )
    }

    pub(crate) fn prompted(&self, session_id: &str, message_id: &str, params: &Value) {
        let listener = lock(&self.state)
            .prompt_listener
            .as_ref()
            .map(|(_, listener)| listener.clone());
        if let Some(listener) = listener {
            listener(session_id, message_id, params);
        }
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
