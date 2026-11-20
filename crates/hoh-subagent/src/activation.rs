//! Capacity and per-child operation serialization.

use std::{collections::HashSet, future::Future, sync::Mutex};

use hoh_error::BoxError;
use hoh_session::SessionId;
use tokio::sync::Mutex as AsyncMutex;

use crate::error::{SubagentError, SubagentErrorCode};

#[derive(Default)]
pub struct ActivationPool {
    next: Mutex<u64>,
    slots: Mutex<HashSet<u64>>,
}

pub struct ActivationSlot<'a> {
    pool: &'a ActivationPool,
    id: Option<u64>,
}

impl ActivationPool {
    pub fn reserve(&self, capacity: usize) -> Result<ActivationSlot<'_>, BoxError> {
        let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        if slots.len() >= capacity {
            return Err(Box::new(SubagentError::error(
                format!(
                    "subagent limit reached (active child limit: {capacity}); wait for an existing child to finish or complete this work with the current agents"
                ),
                SubagentErrorCode::ActivationLimitReached,
            )));
        }
        let mut next = self.next.lock().unwrap_or_else(|p| p.into_inner());
        *next = next.wrapping_add(1);
        let id = *next;
        slots.insert(id);
        Ok(ActivationSlot {
            pool: self,
            id: Some(id),
        })
    }

    pub fn active(&self) -> usize {
        self.slots.lock().unwrap_or_else(|p| p.into_inner()).len()
    }
}

impl ActivationSlot<'_> {
    pub fn release(mut self) {
        if let Some(id) = self.id.take() {
            self.pool
                .slots
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
        }
    }
}

impl Drop for ActivationSlot<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            self.pool
                .slots
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
        }
    }
}

#[derive(Default)]
pub struct ChildLock {
    locks: Mutex<std::collections::HashMap<SessionId, std::sync::Arc<AsyncMutex<()>>>>,
}

impl ChildLock {
    pub async fn run<T, F, Fut>(&self, child_id: SessionId, operation: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let lock = self
            .locks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(child_id)
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        operation().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_error::HarnessError;
    use std::sync::Arc;

    #[test]
    fn capacity_slot_is_released_on_drop_and_explicit_release() {
        let pool = ActivationPool::default();
        let first = pool.reserve(1).unwrap();
        assert_eq!(pool.active(), 1);
        let error = pool.reserve(1).err().unwrap();
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().code(),
            "ACTIVATION_LIMIT_REACHED"
        );
        drop(first);
        let second = pool.reserve(1).unwrap();
        second.release();
        assert_eq!(pool.active(), 0);
    }

    #[tokio::test]
    async fn failed_child_operation_does_not_poison_later_operation() {
        let locks = Arc::new(ChildLock::default());
        let id: SessionId = "child".into();
        let first: Result<(), ()> = locks.run(id.clone(), || async { Err(()) }).await;
        assert!(first.is_err());
        assert_eq!(locks.run(id, || async { 42 }).await, 42);
    }
}
