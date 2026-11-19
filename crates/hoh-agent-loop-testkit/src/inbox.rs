use hoh_agent::{Inbox, InboxState, InboxTarget};
use hoh_error::BoxError;
use hoh_llm::{MessageId, UserMessage};
use std::sync::{Arc, Mutex, MutexGuard};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct InboxStub {
    pending: Mutex<InboxState>,
}

impl InboxStub {
    fn locate(state: &InboxState, id: &MessageId) -> Option<(InboxTarget, usize)> {
        [InboxTarget::NextTurn, InboxTarget::NextStep]
            .into_iter()
            .find_map(|target| {
                state
                    .list(target)
                    .iter()
                    .position(|message| &message.id == id)
                    .map(|index| (target, index))
            })
    }

    fn list_mut(state: &mut InboxState, target: InboxTarget) -> &mut Vec<UserMessage> {
        match target {
            InboxTarget::NextTurn => &mut state.next_turn,
            InboxTarget::NextStep => &mut state.next_step,
        }
    }
}

impl Inbox for InboxStub {
    fn next_turn(&self) -> Vec<UserMessage> {
        lock(&self.pending).next_turn.clone()
    }

    fn next_step(&self) -> Vec<UserMessage> {
        lock(&self.pending).next_step.clone()
    }

    fn clear(&self) -> Result<(), BoxError> {
        let mut pending = lock(&self.pending);
        pending.next_step.clear();
        pending.next_turn.clear();
        Ok(())
    }

    fn append(&self, target: InboxTarget, message: UserMessage) -> Result<(), BoxError> {
        Self::list_mut(&mut lock(&self.pending), target).push(message);
        Ok(())
    }

    fn prepend(&self, target: InboxTarget, message: UserMessage) -> Result<(), BoxError> {
        Self::list_mut(&mut lock(&self.pending), target).insert(0, message);
        Ok(())
    }

    fn replace(&self, id: &MessageId, message: UserMessage) -> Result<bool, BoxError> {
        let mut pending = lock(&self.pending);
        let Some((target, index)) = Self::locate(&pending, id) else {
            return Ok(false);
        };
        Self::list_mut(&mut pending, target)[index] = message;
        Ok(true)
    }

    fn remove(&self, id: &MessageId) -> Result<bool, BoxError> {
        let mut pending = lock(&self.pending);
        let Some((target, index)) = Self::locate(&pending, id) else {
            return Ok(false);
        };
        Self::list_mut(&mut pending, target).remove(index);
        Ok(true)
    }

    fn splice(
        &self,
        target: InboxTarget,
        start: f64,
        delete_count: f64,
        inserted: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError> {
        let mut pending = lock(&self.pending);
        let list = Self::list_mut(&mut pending, target);
        let len = list.len() as f64;
        let start = if start.trunc().is_nan() {
            0.0
        } else {
            start.trunc()
        };
        let start = if start < 0.0 {
            (len + start).max(0.0)
        } else {
            start.min(len)
        } as usize;
        let delete_count = if delete_count.trunc().is_nan() {
            0.0
        } else {
            delete_count.trunc()
        }
        .max(0.0)
        .min((list.len() - start) as f64) as usize;
        let removed = list.drain(start..start + delete_count).collect();
        list.splice(start..start, inserted);
        Ok(removed)
    }
}

/// Create a mutable process-local Inbox backed by two in-memory lists.
pub fn create_inbox_stub() -> Arc<dyn Inbox> {
    Arc::new(InboxStub {
        pending: Mutex::new(InboxState::default()),
    })
}

struct UnsupportedInbox;

impl UnsupportedInbox {
    fn reject<T>() -> Result<T, BoxError> {
        Err("this test Agent does not support Inbox mutations".into())
    }
}

impl Inbox for UnsupportedInbox {
    fn next_turn(&self) -> Vec<UserMessage> {
        vec![]
    }
    fn next_step(&self) -> Vec<UserMessage> {
        vec![]
    }
    fn clear(&self) -> Result<(), BoxError> {
        Self::reject()
    }
    fn append(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Self::reject()
    }
    fn prepend(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Self::reject()
    }
    fn replace(&self, _: &MessageId, _: UserMessage) -> Result<bool, BoxError> {
        Self::reject()
    }
    fn remove(&self, _: &MessageId) -> Result<bool, BoxError> {
        Self::reject()
    }
    fn splice(
        &self,
        _: InboxTarget,
        _: f64,
        _: f64,
        _: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError> {
        Self::reject()
    }
}

/// Create a fail-fast Inbox placeholder for structural Agent stubs.
pub fn unsupported_inbox() -> Arc<dyn Inbox> {
    Arc::new(UnsupportedInbox)
}
