//! The in-process face of the contract: what a harness package implements and registers.

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::sync::mpsc;

use crate::contract::{
    AccessMode, Discovery, HarnessEvent, HarnessFailure, HarnessPolicy, PromptParams,
    RespondParams, StartParams, StartResult, VersionStatus, code,
};

pub type HarnessResult<T> = Result<T, HarnessFailure>;

/// One kind of external harness (a CLI and its protocol), registered under a name through
/// [`crate::ExternalHarnessesService::register`].
pub trait ExternalHarness: Send + Sync + 'static {
    /// Installed, signed in, models — without reading secrets.
    fn discover(&self) -> BoxFuture<'static, HarnessResult<Discovery>>;
    /// Start a native session for one HoH session, or continue `params.resume`. A `resume` the
    /// harness no longer knows fails with [`crate::code::SESSION_NOT_FOUND`].
    fn start_or_resume(&self, params: StartParams) -> BoxFuture<'static, HarnessResult<Started>>;

    /// [`start_or_resume`](Self::start_or_resume) under the access mode the user chose in the
    /// window; the slot starts every native session through this. The default starts it as
    /// `start_or_resume` unless [`AccessMode::Full`] would widen it ([`AccessMode::widens`]): a
    /// harness that does not say how it runs without questions is not run so.
    fn start_with_access(
        &self,
        params: StartParams,
        access: AccessMode,
    ) -> BoxFuture<'static, HarnessResult<Started>> {
        if access.widens(&params.policy) {
            return Box::pin(std::future::ready(Err(HarnessFailure::new(
                code::POLICY_UNAVAILABLE,
                "this harness has no full-access launch",
            ))));
        }
        self.start_or_resume(params)
    }

    /// The command-line switches that decide what a native session started with `policy` under
    /// `access` may do without asking (permission mode, permission prompt, tool limits), exactly
    /// as [`start_with_access`](Self::start_with_access) passes them: the window shows the mode
    /// from them. No ids, no secrets. Default: none.
    fn launch_flags(&self, policy: &HarnessPolicy, access: AccessMode) -> Vec<String> {
        let _ = (policy, access);
        Vec::new()
    }

    /// Whether «Стоп» ends the process of the session, with every process under it, instead of
    /// trusting the native interrupt: for a harness whose interrupt may leave a command running.
    /// The next turn starts the process again on the same native session. Default: no.
    fn stop_ends_process(&self) -> bool {
        false
    }

    /// The effort levels the harness takes in [`StartParams::effort`], lowest first: the window
    /// names one in `session/prompt` (`effort`, HOH-40), and another level than the running
    /// process has starts the session anew. Default: none, so a level is not passed.
    fn effort_levels(&self) -> &'static [&'static str] {
        &[]
    }

    /// One isolated, no-tools title request. The default harness does not provide it.
    fn generate_title(&self, prompt: String) -> BoxFuture<'static, HarnessResult<String>> {
        let _ = prompt;
        Box::pin(std::future::ready(Err(HarnessFailure::new(
            code::HARNESS,
            "this harness cannot generate conversation titles",
        ))))
    }

    /// [`discover`](Self::discover), and where the found version stands against the range the
    /// harness was checked with. Default: the range is not known.
    fn discover_with_status(
        &self,
    ) -> BoxFuture<'static, HarnessResult<(Discovery, VersionStatus)>> {
        let discovery = self.discover();
        Box::pin(async move { Ok((discovery.await?, VersionStatus::Unknown)) })
    }
}

/// A running native session: its control half and its events.
pub struct Started {
    pub result: StartResult,
    pub session: Arc<dyn HarnessSession>,
    /// Bounded: a slow reader holds the harness back instead of growing memory. The stream ends
    /// after [`HarnessEvent::SessionEnded`] (or without it when the session was closed cleanly).
    pub events: mpsc::Receiver<HarnessEvent>,
}

/// Control of one running native session.
pub trait HarnessSession: Send + Sync + 'static {
    fn prompt(&self, params: PromptParams) -> BoxFuture<'static, HarnessResult<()>>;
    /// The native interrupt of the running turn; the turn then ends with
    /// [`HarnessEvent::TurnAborted`].
    fn interrupt(&self) -> BoxFuture<'static, HarnessResult<()>>;
    fn respond(&self, params: RespondParams) -> BoxFuture<'static, HarnessResult<()>>;
    /// End the session for good: a graceful stop, then for a process SIGTERM to its group and,
    /// after `grace`, killing the group. Idempotent.
    fn terminate(&self, grace: Duration) -> BoxFuture<'static, ()>;
    /// Recent diagnostics (stderr of a process), with secrets masked.
    fn diagnostics(&self) -> String;
}
