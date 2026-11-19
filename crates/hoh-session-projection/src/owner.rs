//! Synchronous source operations serialize across threads and may re-enter on
//! the same thread. The ownership mutex is released before user code runs.
use std::{
    marker::PhantomData,
    rc::Rc,
    sync::{Condvar, Mutex},
    thread::ThreadId,
};

#[derive(Default)]
struct Ownership {
    thread: Option<ThreadId>,
    depth: usize,
}
#[derive(Default)]
pub(crate) struct Owner {
    state: Mutex<Ownership>,
    changed: Condvar,
}
pub(crate) struct Guard<'a> {
    owner: &'a Owner,
    _same_thread: PhantomData<Rc<()>>,
}
impl Owner {
    pub(crate) fn enter(&self) -> Guard<'_> {
        let me = std::thread::current().id();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.thread.is_some_and(|owner| owner != me) {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.thread = Some(me);
        state.depth += 1;
        Guard {
            owner: self,
            _same_thread: PhantomData,
        }
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let mut state = self
            .owner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.depth -= 1;
        if state.depth == 0 {
            state.thread = None;
            self.owner.changed.notify_one();
        }
    }
}
