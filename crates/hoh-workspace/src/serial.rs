use std::sync::{Mutex, MutexGuard};

#[derive(Default)]
pub(crate) struct OperationQueue {
    state: Mutex<QueueState>,
    changed: tokio::sync::Notify,
}

#[derive(Default)]
struct QueueState {
    next: u64,
    serving: u64,
}

impl OperationQueue {
    /// Reserve source-order synchronously when the caller's future is first polled.
    pub(crate) fn reserve(&self) -> u64 {
        let mut state = lock(&self.state);
        let ticket = state.next;
        state.next = state
            .next
            .checked_add(1)
            .expect("workspace operation ticket overflow");
        ticket
    }

    pub(crate) async fn enter(&self, ticket: u64) -> OperationTurn<'_> {
        loop {
            // Register before checking so a completion between the check and await
            // cannot be lost.
            let changed = self.changed.notified();
            if lock(&self.state).serving == ticket {
                return OperationTurn {
                    queue: self,
                    ticket,
                };
            }
            changed.await;
        }
    }
}

pub(crate) struct OperationTurn<'a> {
    queue: &'a OperationQueue,
    ticket: u64,
}

impl Drop for OperationTurn<'_> {
    fn drop(&mut self) {
        {
            let mut state = lock(&self.queue.state);
            debug_assert_eq!(state.serving, self.ticket);
            state.serving = state
                .serving
                .checked_add(1)
                .expect("workspace operation ticket overflow");
        }
        self.queue.changed.notify_waiters();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
