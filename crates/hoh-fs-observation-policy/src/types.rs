//! Actor vocabulary used to derive the owner of filesystem observations.

use hoh_session::Session;

/// Minimal agent-shaped portion of an observation actor.
#[derive(Clone, Default)]
pub struct FsObservationAgent {
    /// The session that owns observed-file state.
    pub session: Option<Session>,
}

/// Minimal direct actor used by event callers that do not carry `ToolExecution`.
///
/// Production `hoh-tool-fs` actors are downcast from `ToolExecution`; this type
/// preserves the source package's public structural actor vocabulary.
#[derive(Clone, Default)]
pub struct FsObservationActor {
    /// The calling agent, when one exists.
    pub agent: Option<FsObservationAgent>,
}

impl FsObservationActor {
    /// Borrow the owner session, if the complete actor chain is present.
    pub fn session(&self) -> Option<Session> {
        self.agent.as_ref()?.session.clone()
    }
}
