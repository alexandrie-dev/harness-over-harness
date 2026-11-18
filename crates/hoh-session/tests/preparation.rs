mod common;
use common::*;
use hoh_session::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
#[test]
fn retains_the_exact_session_and_releases_provider_state_only_once() {
    let s = session();
    let count = Arc::new(AtomicUsize::new(0));
    {
        let c = count.clone();
        let mut p = SessionPreparation::create(
            s.clone(),
            SessionPreparationOptions {
                release: Some(Box::new(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                })),
            },
        );
        assert_eq!(p.session, s);
        p.dispose();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
}
#[test]
fn disposes_a_preparation_without_provider_state() {
    let mut p = SessionPreparation::create(session(), Default::default());
    p.dispose();
    p.dispose();
}
