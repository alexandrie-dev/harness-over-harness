use crate::types::{McpCloseResult, McpConnectionStatus};
use std::sync::{Arc, Mutex, MutexGuard};

type Listener = Arc<dyn Fn(McpConnectionStatus) + Send + Sync>;
type WarningSink = Arc<dyn Fn(&str) + Send + Sync>;
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub trait McpConnectionStatusHandle: Send + Sync {
    fn current(&self) -> McpConnectionStatus;
    fn closure(&self) -> McpCloseResult;
    fn on_change(&self, callback: Listener) -> StatusListener;
}

struct State {
    value: McpConnectionStatus,
    close: McpCloseResult,
    listeners: Vec<(u64, Listener)>,
    next_id: u64,
}

#[derive(Clone)]
pub struct McpStatusCell {
    state: Arc<Mutex<State>>,
    warn: Arc<Mutex<WarningSink>>,
}

impl McpStatusCell {
    pub fn new(warn: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                value: McpConnectionStatus::Connecting { attempt: 0 },
                close: McpCloseResult {
                    close_confirmed: true,
                    cleanup_error: None,
                },
                listeners: Vec::new(),
                next_id: 0,
            })),
            warn: Arc::new(Mutex::new(Arc::new(warn))),
        }
    }
    pub fn set_warning_sink(&self, warn: impl Fn(&str) + Send + Sync + 'static) {
        *lock(&self.warn) = Arc::new(warn);
    }
    pub fn opened(&self) {
        lock(&self.state).close = McpCloseResult {
            close_confirmed: false,
            cleanup_error: None,
        };
    }
    pub fn record_closure(&self, result: McpCloseResult) {
        lock(&self.state).close = result;
    }
    pub fn detach(&self) {
        lock(&self.state).listeners.clear();
    }
    pub fn update(&self, next: McpConnectionStatus) {
        let listeners = {
            let mut state = lock(&self.state);
            state.value = next.clone();
            state
                .listeners
                .iter()
                .map(|(_, listener)| listener.clone())
                .collect::<Vec<_>>()
        };
        for listener in listeners {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(next.clone())))
                .is_err()
            {
                lock(&self.warn)("an onChange status observer failed");
            }
        }
    }
}

impl McpConnectionStatusHandle for McpStatusCell {
    fn current(&self) -> McpConnectionStatus {
        lock(&self.state).value.clone()
    }
    fn closure(&self) -> McpCloseResult {
        lock(&self.state).close.clone()
    }
    fn on_change(&self, callback: Listener) -> StatusListener {
        let id = {
            let mut state = lock(&self.state);
            let id = state.next_id;
            state.next_id += 1;
            state.listeners.push((id, callback));
            id
        };
        StatusListener {
            state: Arc::downgrade(&self.state),
            id,
        }
    }
}

pub struct StatusListener {
    state: std::sync::Weak<Mutex<State>>,
    id: u64,
}
impl Drop for StatusListener {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            lock(&state).listeners.retain(|(id, _)| *id != self.id);
        }
    }
}
