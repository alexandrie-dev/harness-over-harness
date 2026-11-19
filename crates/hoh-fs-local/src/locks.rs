//! Eager FIFO first-key reservation, then lexical multi-key acquisition.
use crate::lock;
use hoh_error::BoxError;
use hoh_fs::FsFuture;
use indexmap::IndexMap;
use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::{oneshot, watch};
struct Tail {
    done: watch::Receiver<bool>,
}
#[derive(Default)]
pub(crate) struct Locks {
    tails: Mutex<IndexMap<String, Arc<Tail>>>,
}
struct Reservation {
    key: String,
    tail: Arc<Tail>,
    done: watch::Sender<bool>,
}
impl Locks {
    pub(crate) fn len(&self) -> usize {
        lock(&self.tails).len()
    }
    fn reserve(&self, key: String) -> (Option<watch::Receiver<bool>>, Reservation) {
        let mut tails = lock(&self.tails);
        let prior = tails.get(&key).map(|t| t.done.clone());
        let (done, receiver) = watch::channel(false);
        let tail = Arc::new(Tail { done: receiver });
        tails.insert(key.clone(), tail.clone());
        (prior, Reservation { key, tail, done })
    }
    pub(crate) fn run<T: Send + 'static>(
        self: &Arc<Self>,
        mut keys: Vec<String>,
        operation: impl Future<Output = Result<T, BoxError>> + Send + 'static,
    ) -> FsFuture<'static, T> {
        keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        keys.dedup();
        // All mutations carry at least one canonical target key.
        let first = keys.first().expect("mutation has a target key").clone();
        let (prior, reservation) = self.reserve(first);
        let state = self.clone();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let mut held = Held {
                state: state.clone(),
                reservations: vec![reservation],
            };
            wait(prior).await;
            for key in keys.into_iter().skip(1) {
                let (prior, reservation) = state.reserve(key);
                held.reservations.push(reservation);
                wait(prior).await;
            }
            let result = operation.await;
            drop(held);
            let _ = tx.send(result);
        });
        Box::pin(async move {
            rx.await
                .map_err(|_| std::io::Error::other("filesystem mutation task stopped"))?
        })
    }
}
async fn wait(prior: Option<watch::Receiver<bool>>) {
    if let Some(mut prior) = prior {
        while !*prior.borrow_and_update() {
            if prior.changed().await.is_err() {
                break;
            }
        }
    }
}
struct Held {
    state: Arc<Locks>,
    reservations: Vec<Reservation>,
}
impl Drop for Held {
    fn drop(&mut self) {
        for reservation in self.reservations.drain(..).rev() {
            {
                let mut tails = lock(&self.state.tails);
                if tails
                    .get(&reservation.key)
                    .is_some_and(|current| Arc::ptr_eq(current, &reservation.tail))
                {
                    tails.shift_remove(&reservation.key);
                }
            }
            let _ = reservation.done.send(true);
        }
    }
}
