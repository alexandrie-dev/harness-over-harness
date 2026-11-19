use futures_util::future::{BoxFuture, Shared};
use hoh_error::SharedError;
use hoh_timeout::AbortSignal;

pub type RangeExit = Shared<BoxFuture<'static, Result<(), SharedError>>>;
pub trait BoundProcessOwner: Send + Sync {
    fn signal(&self, signal: rustix::process::Signal);
    fn wait_for_exit(&self) -> RangeExit;
    fn inspect_task_count(&self) -> Result<Option<u64>, hoh_error::BoxError> {
        Ok(None)
    }
    fn terminate_for_host_exit(&self) {
        self.signal(rustix::process::Signal::KILL);
    }
    fn cleanup(&self) -> Result<(), hoh_error::BoxError> {
        Ok(())
    }
}
pub async fn wait_with_abort(
    pending: RangeExit,
    signal: Option<AbortSignal>,
) -> Result<bool, SharedError> {
    if let Some(signal) = signal {
        if signal.aborted() {
            return Ok(false);
        }
        tokio::select! { biased; _ = signal.cancelled() => Ok(false), result = pending => result.map(|()|true) }
    } else {
        pending.await.map(|()| true)
    }
}
