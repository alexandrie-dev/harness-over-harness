//! The registry-owned pull pump: copies a job's output sources into its ring at a bounded cadence
//! and drains them once more after the producer settles, so the ring holds every byte before
//! settlement trims and closes it.
//!
//! Port of `src/pump.ts`.

use hoh_error::BoxError;
use hoh_jobs::{JobAppendOptions, JobOutputSource};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::task::JoinHandle;

/// Where a pump delivers what it reads.
pub trait PumpSink: Send + Sync + 'static {
    /// Append copied text to the ring.
    fn append(&self, text: &str, options: JobAppendOptions);
    /// Record the spill file source `index` advertised on its latest read; called on every read,
    /// with `None` when the source keeps none or withdrew it.
    fn spill(&self, index: usize, path: Option<&str>);
}

/// One pump run; `done` resolves after the final post-settlement drain.
pub struct PumpHandle {
    pub done: JoinHandle<Result<(), BoxError>>,
}

struct SourceState {
    source: JobOutputSource,
    cursor: u64,
}

fn drain(states: &mut [SourceState], sink: &dyn PumpSink) -> Result<(), BoxError> {
    for (index, state) in states.iter_mut().enumerate() {
        let read = (state.source.read)(state.cursor)?;
        state.cursor = read.next_offset;
        sink.spill(index, read.spill_path.as_deref());
        if read.text.is_empty() {
            continue;
        }
        if state.source.channel.is_none() && !read.lossy {
            sink.append(&read.text, JobAppendOptions::default());
        } else {
            sink.append(
                &read.text,
                JobAppendOptions {
                    channel: state.source.channel,
                    gap_before: read.lossy,
                },
            );
        }
    }
    Ok(())
}

/// `JSON.stringify` of a number, as the source's messages print it.
pub fn json_number(value: f64) -> String {
    if value.is_finite() {
        hoh_util_values::js_number_string(value)
    } else {
        "null".to_owned()
    }
}

/// Drain every source in array order, sleep `poll_ms` or until `until` settles, repeat, then drain
/// one final time. The first drain runs before this function returns.
///
/// # Errors
/// A `poll_ms` that is not a positive finite number, or a failed first read.
pub fn start_pump(
    sources: Vec<JobOutputSource>,
    sink: Arc<dyn PumpSink>,
    poll_ms: f64,
    until: impl Future<Output = ()> + Send + 'static,
) -> Result<PumpHandle, BoxError> {
    if !poll_ms.is_finite() || poll_ms <= 0.0 {
        return Err(format!(
            "invalid pump pollMs: expected a positive finite number of milliseconds, got {}",
            json_number(poll_ms)
        )
        .into());
    }
    let mut states: Vec<SourceState> = sources
        .into_iter()
        .map(|source| SourceState { source, cursor: 0 })
        .collect();
    let first = drain(&mut states, sink.as_ref());
    let interval = Duration::from_secs_f64(poll_ms / 1000.0);
    let done = tokio::spawn(async move {
        first?;
        let mut until = Box::pin(until);
        loop {
            let settled = tokio::select! {
                () = &mut until => true,
                () = tokio::time::sleep(interval) => false,
            };
            if settled {
                break;
            }
            drain(&mut states, sink.as_ref())?;
        }
        drain(&mut states, sink.as_ref())
    });
    Ok(PumpHandle { done })
}
