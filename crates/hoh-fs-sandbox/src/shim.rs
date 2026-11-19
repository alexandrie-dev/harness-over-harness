//! Temporary interface for the sandbox-policy package being ported concurrently.

use hoh_sandbox::SandboxExecutionPolicy;
use hoh_session::Session;

/// SHIM(hoh-sandbox-policy): replace with the accepted crate's policy service.
pub trait SandboxPolicyApi: Send + Sync + 'static {
    /// Resolve the effective policy for a session, or the deployment fallback.
    fn resolve(&self, session: Option<&Session>) -> SandboxExecutionPolicy;
}

hoh_kernel::service!(pub SandboxPolicyKey: dyn SandboxPolicyApi = "sandboxPolicy");
