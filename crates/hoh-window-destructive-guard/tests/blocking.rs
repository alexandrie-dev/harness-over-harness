//! The classifier's file system checks run on a blocking thread, never on the runtime thread.
//! The seam blocks like a slow mount; a heartbeat task on the same single-threaded runtime must
//! keep ticking meanwhile (if the checks ran inline, the seam would wait for ticks that can only
//! come after it returns).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use hoh_window_destructive_guard::{Fs, decide};

struct SlowFs {
    ticks: Arc<AtomicUsize>,
    runtime_stayed_free: AtomicBool,
    existing: &'static str,
    entries: &'static [&'static str],
}

impl SlowFs {
    /// Block until the heartbeat has ticked three times (or give up after three seconds).
    fn stall(&self) {
        let start = Instant::now();
        while self.ticks.load(Ordering::SeqCst) < 3 {
            if start.elapsed() > Duration::from_secs(3) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        self.runtime_stayed_free.store(true, Ordering::SeqCst);
    }
}

impl Fs for SlowFs {
    fn exists(&self, path: &Path) -> bool {
        self.stall();
        path.ends_with(self.existing)
    }

    fn is_dir(&self, _: &Path) -> bool {
        self.stall();
        false
    }

    fn list(&self, _: &Path) -> Option<Vec<String>> {
        self.stall();
        Some(self.entries.iter().map(|n| (*n).to_owned()).collect())
    }
}

async fn run(command: &str, fs: SlowFs) -> (bool, bool) {
    let ticks = fs.ticks.clone();
    let heartbeat = tokio::spawn(async move {
        loop {
            ticks.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let fs = Arc::new(fs);
    let asked = decide(command, Some("/work".into()), fs.clone())
        .await
        .is_some();
    heartbeat.abort();
    (asked, fs.runtime_stayed_free.load(Ordering::SeqCst))
}

fn slow(existing: &'static str, entries: &'static [&'static str]) -> SlowFs {
    SlowFs {
        ticks: Arc::new(AtomicUsize::new(0)),
        runtime_stayed_free: AtomicBool::new(false),
        existing,
        entries,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_existence_check_does_not_hold_the_runtime_thread() {
    let (asked, free) = run("echo hi > a.txt", slow("a.txt", &[])).await;
    assert!(free, "the file system check blocked the runtime thread");
    assert!(asked, "an overwrite of an existing file must still ask");
}

#[tokio::test(flavor = "current_thread")]
async fn expanding_a_mask_does_not_hold_the_runtime_thread() {
    let (asked, free) = run("sed -i s/a/b/ *.txt", slow("a.txt", &["a.txt", "b.md"])).await;
    assert!(free, "reading the folder blocked the runtime thread");
    assert!(asked);
}
