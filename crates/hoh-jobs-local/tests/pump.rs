//! Port of `tests/pump.spec.ts`. The fake-timer cases run under tokio's paused clock.

use hoh_jobs::{JobAppendOptions, JobChannel, JobOutputSource, JobSourceRead};
use hoh_jobs_local::pump::{PumpSink, start_pump};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::oneshot;

type Append = (String, JobAppendOptions);

#[derive(Default)]
struct Recorder {
    appends: Mutex<Vec<Append>>,
    spills: Mutex<Vec<(usize, Option<String>)>>,
}

impl PumpSink for Recorder {
    fn append(&self, text: &str, options: JobAppendOptions) {
        self.appends
            .lock()
            .unwrap()
            .push((text.to_owned(), options));
    }
    fn spill(&self, index: usize, path: Option<&str>) {
        self.spills
            .lock()
            .unwrap()
            .push((index, path.map(str::to_owned)));
    }
}

#[derive(Default, Clone)]
struct Scripted {
    text: &'static str,
    lossy: bool,
    spill: Option<&'static str>,
}

fn read(text: &'static str) -> Scripted {
    Scripted {
        text,
        ..Scripted::default()
    }
}

/// A scripted source: each read shifts the next scripted result.
fn scripted(
    reads: Vec<Scripted>,
    channel: Option<JobChannel>,
) -> (JobOutputSource, Arc<Mutex<Vec<u64>>>) {
    let offsets = Arc::new(Mutex::new(Vec::new()));
    let queue = Mutex::new(VecDeque::from(reads));
    let offset = Mutex::new(0u64);
    let seen = offsets.clone();
    let source = JobOutputSource {
        channel,
        read: Box::new(move |from| {
            seen.lock().unwrap().push(from);
            let next = queue.lock().unwrap().pop_front().unwrap_or_default();
            let mut offset = offset.lock().unwrap();
            *offset += next.text.len() as u64;
            Ok(JobSourceRead {
                text: next.text.to_owned(),
                next_offset: *offset,
                lossy: next.lossy,
                spill_path: next.spill.map(str::to_owned),
            })
        }),
    };
    (source, offsets)
}

fn opts(channel: Option<JobChannel>, gap_before: bool) -> JobAppendOptions {
    JobAppendOptions {
        channel,
        gap_before,
    }
}

fn appended(sink: &Recorder) -> Vec<Append> {
    sink.appends.lock().unwrap().clone()
}

async fn settle_after_poll(poll_ms: u64) {
    // Strictly past the poll deadline, so the pump's own timer fires first (as the fake clock does).
    tokio::time::sleep(Duration::from_millis(poll_ms + 1)).await;
}

#[tokio::test(start_paused = true)]
async fn copies_labeled_deltas_at_the_poll_cadence_and_resumes_each_source_at_its_own_offset() {
    let sink = Arc::new(Recorder::default());
    let (settle, until) = oneshot::channel::<()>();
    let (out, out_offsets) = scripted(vec![read("a"), read("bc")], Some(JobChannel::Stdout));
    let (err, err_offsets) = scripted(vec![read(""), read("E")], Some(JobChannel::Stderr));
    let pump = start_pump(vec![out, err], sink.clone(), 50.0, async move {
        let _ = until.await;
    })
    .unwrap();

    settle_after_poll(50).await;
    settle.send(()).unwrap();
    pump.done.await.unwrap().unwrap();
    assert_eq!(
        appended(&sink),
        vec![
            ("a".to_owned(), opts(Some(JobChannel::Stdout), false)),
            ("bc".to_owned(), opts(Some(JobChannel::Stdout), false)),
            ("E".to_owned(), opts(Some(JobChannel::Stderr), false)),
        ]
    );
    // Every read resumed from the previous nextOffset, never from a shared cursor.
    assert_eq!(*out_offsets.lock().unwrap(), vec![0, 1, 3]);
    assert_eq!(*err_offsets.lock().unwrap(), vec![0, 0, 1]);
}

#[tokio::test]
async fn marks_a_lossy_source_read_as_a_gap_so_observers_see_the_discontinuity() {
    let sink = Arc::new(Recorder::default());
    let (source, _) = scripted(
        vec![Scripted {
            text: "tail",
            lossy: true,
            spill: None,
        }],
        None,
    );
    start_pump(vec![source], sink.clone(), 1.0, async {})
        .unwrap()
        .done
        .await
        .unwrap()
        .unwrap();
    assert_eq!(appended(&sink), vec![("tail".to_owned(), opts(None, true))]);
}

#[tokio::test(start_paused = true)]
async fn reports_each_sources_spill_file_on_every_read_and_withdraws_it_once_a_read_stops_naming_it()
 {
    let sink = Arc::new(Recorder::default());
    let (settle, until) = oneshot::channel::<()>();
    let (out, _) = scripted(
        vec![
            Scripted {
                text: "tail",
                lossy: true,
                spill: Some("/spill/out.log"),
            },
            Scripted {
                text: "more",
                lossy: false,
                spill: Some("/spill/out.log"),
            },
            read(""),
        ],
        Some(JobChannel::Stdout),
    );
    let (err, _) = scripted(
        vec![
            read(""),
            Scripted {
                text: "E",
                lossy: false,
                spill: Some("/spill/err.log"),
            },
        ],
        Some(JobChannel::Stderr),
    );
    let pump = start_pump(vec![out, err], sink.clone(), 50.0, async move {
        let _ = until.await;
    })
    .unwrap();

    settle_after_poll(50).await;
    settle.send(()).unwrap();
    pump.done.await.unwrap().unwrap();
    // Chunks carry the gap marker only; the file reference is source metadata.
    assert_eq!(
        appended(&sink),
        vec![
            ("tail".to_owned(), opts(Some(JobChannel::Stdout), true)),
            ("more".to_owned(), opts(Some(JobChannel::Stdout), false)),
            ("E".to_owned(), opts(Some(JobChannel::Stderr), false)),
        ]
    );
    // Every read reports, lossy or clean, empty or not: the third stdout read withdraws the file.
    let some = |path: &str| Some(path.to_owned());
    assert_eq!(
        *sink.spills.lock().unwrap(),
        vec![
            (0, some("/spill/out.log")),
            (1, None),
            (0, some("/spill/out.log")),
            (1, some("/spill/err.log")),
            (0, None),
            (1, None),
        ]
    );
}

#[tokio::test]
async fn treats_a_failed_settlement_as_settlement_and_still_drains_the_final_bytes() {
    // The Rust `until` future carries no outcome, so a producer failure resolves it the same way.
    let sink = Arc::new(Recorder::default());
    let (source, _) = scripted(vec![read("last")], None);
    start_pump(vec![source], sink.clone(), 1.0, async {
        let failed: Result<(), &str> = Err("producer broke");
        let _ = failed;
    })
    .unwrap()
    .done
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        appended(&sink),
        vec![("last".to_owned(), opts(None, false))]
    );
}

#[tokio::test(start_paused = true)]
async fn holds_one_poll_timer_however_long_the_job_runs_and_reads_once_per_round() {
    let sink = Arc::new(Recorder::default());
    let (settle, until) = oneshot::channel::<()>();
    let (source, offsets) = scripted(vec![], None);
    let pump = start_pump(vec![source], sink, 50.0, async move {
        let _ = until.await;
    })
    .unwrap();

    let rounds: u64 = 10_000;
    tokio::time::sleep(Duration::from_millis(50 * rounds + 1)).await;
    assert_eq!(offsets.lock().unwrap().len() as u64, rounds + 1);

    settle.send(()).unwrap();
    pump.done.await.unwrap().unwrap();
    assert_eq!(offsets.lock().unwrap().len() as u64, rounds + 2);
}

#[tokio::test]
async fn rejects_a_non_positive_poll_interval_before_touching_any_source() {
    let sink = Arc::new(Recorder::default());
    let error = start_pump(vec![], sink, 0.0, async {})
        .err()
        .expect("must reject");
    assert!(error.to_string().contains("invalid pump pollMs"));
}
