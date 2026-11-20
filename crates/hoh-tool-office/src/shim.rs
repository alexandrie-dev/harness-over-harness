//! Fixed interfaces for owner packages that do not expose their source API yet.

use hoh_error::BoxError;
use hoh_session::{Session, SessionId};

/// SHIM(hoh-subagent): resolve the root Session of an Agent tree for output ownership.
/// Absence means the calling Session owns its files, as in the source package.
pub trait RootSessionResolver: Send + Sync + 'static {
    fn root_of(&self, session: &Session) -> Result<SessionId, BoxError>;
}

hoh_kernel::service!(pub Subagents: dyn RootSessionResolver = "subagents");
