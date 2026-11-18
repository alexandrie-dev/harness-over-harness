//! Port of @deepseek-ai/dsh-subprocess (packages/subprocess/subprocess).
//! Provider-independent subprocess service, vocabulary, errors and child environment policy.

pub mod control;
pub mod error;
pub mod types;

use hoh_http_proxy::{ProxyInstallation, proxy_environment_for_child};
use indexmap::IndexMap;
use std::sync::Arc;

pub use error::{
    SUBPROCESS_ERROR_CODES, SubprocessError, SubprocessErrorCode,
    SubprocessExecutableNotFoundError, SubprocessFailure, SubprocessForeignError,
    subprocess_spawn_failure,
};
pub use types::*;

/// Source of the original JavaScript regular expression, with ASCII case-insensitive matching.
pub const SENSITIVE_ENV_PATTERN: &str = "KEY|PASSWORD|SECRET|TOKEN";

/// Read the ambient environment lossily, as Node does, and apply the explicit proxy installation.
pub fn scrubbed_parent_env(proxy: Option<&ProxyInstallation>) -> IndexMap<String, String> {
    scrubbed_parent_env_from(
        std::env::vars_os().map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                Some(value.to_string_lossy().into_owned()),
            )
        }),
        proxy,
    )
}

/// The same scrub over a caller-owned snapshot; `None` represents an absent environment entry.
pub fn scrubbed_parent_env_from(
    parent: impl IntoIterator<Item = (String, Option<String>)>,
    proxy: Option<&ProxyInstallation>,
) -> IndexMap<String, String> {
    let mut environment: SubprocessEnvironment = parent.into_iter().collect();
    // In Node this publication happens before process.env is scrubbed. Rust passes it explicitly.
    if let Some(proxy) = proxy {
        apply_overlay(&mut environment, proxy.environment());
    }
    let mut result: IndexMap<String, String> = environment
        .into_iter()
        .filter_map(|(name, value)| {
            let ascii = name.to_ascii_uppercase();
            let sensitive = SENSITIVE_ENV_PATTERN
                .split('|')
                .any(|needle| ascii.contains(needle));
            (!sensitive && !name.to_uppercase().starts_with(DSH_ENV_PREFIX))
                .then(|| value.map(|value| (name, value)))
                .flatten()
        })
        .collect();
    for (name, value) in proxy_environment_for_child(proxy) {
        match value {
            Some(value) => {
                result.insert(name, value);
            }
            None => {
                result.shift_remove(&name);
            }
        }
    }
    result
}

fn apply_overlay(environment: &mut SubprocessEnvironment, overlay: &SubprocessEnvironment) {
    for (name, value) in overlay {
        match value {
            Some(value) => {
                environment.insert(name.clone(), Some(value.clone()));
            }
            None => {
                environment.shift_remove(name);
            }
        }
    }
}

/// Abstract provider contract. Providers own process creation, termination, collection and teardown.
/// Futures are returned synchronously so providers can initiate work before the first poll.
pub trait SubprocessRuntime: Send + Sync + 'static {
    fn resolve_executable<'a>(
        &'a self,
        command: &'a str,
        env: Option<&'a IndexMap<String, String>>,
        signal: Option<hoh_timeout::AbortSignal>,
    ) -> SubprocessFuture<'a, String>;

    fn terminal_environment(
        &self,
        signal: Option<hoh_timeout::AbortSignal>,
    ) -> SubprocessFuture<'_, SubprocessTerminalEnvironment>;

    /// Synchronous validation and handle publication; provider failures may later reject `done`.
    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<SubprocessHandle, hoh_error::BoxError>;

    fn spawn_terminal(
        &self,
        spec: SubprocessTerminalSpawnSpec,
    ) -> SubprocessFuture<'_, SubprocessTerminalHandle>;
}

hoh_kernel::service!(pub Subprocess: dyn SubprocessRuntime = "subprocess");

/// Register one provider with the kernel's ordinary duplicate-service and lifecycle semantics.
pub fn provide_subprocess(
    ctx: &hoh_kernel::Context,
    runtime: Arc<dyn SubprocessRuntime>,
) -> impl Future<Output = Result<hoh_kernel::Disposer, hoh_kernel::KernelError>> + Send + 'static {
    ctx.provide::<Subprocess>(runtime)
}
