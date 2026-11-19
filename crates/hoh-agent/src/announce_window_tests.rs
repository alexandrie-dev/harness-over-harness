//! The window of `announce` between finding its entry and marking it is closed: the registry
//! lock, which `detach` needs to remove the entry, is held across it.
#[path = "../tests/common/mod.rs"]
mod common;

use super::*;
use common::{mount, stub_agent};
use std::sync::atomic::AtomicBool;

#[tokio::test]
async fn announce_holds_the_registry_lock_from_finding_its_entry_to_marking_it() {
    let h = mount().await;
    let agents = (*h.ctx.get::<Agents>().unwrap()).clone();
    let held = Arc::new(AtomicBool::new(false));
    let probed = Arc::new(AtomicBool::new(false));
    let (flag, seen) = (held.clone(), probed.clone());
    agents.set_window_probe(Arc::new(move |store| {
        // `detach` removes the entry under this lock: while it is unavailable no detach can run.
        flag.store(store.try_lock().is_err(), Ordering::SeqCst);
        seen.store(true, Ordering::SeqCst);
    }));
    let agent = stub_agent(&h.ctx, "window");
    let _detach = agents.enter(&h.ctx, agent.clone()).unwrap();
    agents
        .announce(&h.ctx, &agent, SessionStartSource::Startup, None)
        .await
        .unwrap();
    assert!(probed.load(Ordering::SeqCst), "the window was not reached");
    assert!(
        held.load(Ordering::SeqCst),
        "the registry lock was free in the window"
    );
    h.kernel.shutdown().await;
}
