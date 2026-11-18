//! The inherited endpoint is a byte stream: this module adds no framing or messages.

use crate::types::{SubprocessDuplex, SubprocessEnvironment};
use hoh_error::BoxError;

pub const SUBPROCESS_CONTROL_FD: u32 = 7;
pub const SUBPROCESS_CONTROL_ENV: &str = "DSH_SUBPROCESS_CONTROL";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlChannelOptions {
    pub fd: u32,
    pub readable: bool,
    pub writable: bool,
    pub allow_half_open: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ControlChannelError {
    #[error("subprocess control channel was not inherited")]
    NotInherited,
}

/// Consume a launch snapshot's marker, then ask the launcher's owned-descriptor opener for fd 7.
/// The opener transfers ownership of a duplex stream, preserving independent read/write halves.
/// No raw descriptor is adopted with unsafe code and no process-global environment is mutated.
pub fn open_inherited_control_channel<S: SubprocessDuplex>(
    environment: &mut SubprocessEnvironment,
    open: impl FnOnce(ControlChannelOptions) -> Result<S, BoxError>,
) -> Result<S, BoxError> {
    #[cfg(windows)]
    let key = environment
        .keys()
        .find(|key| key.eq_ignore_ascii_case(SUBPROCESS_CONTROL_ENV))
        .cloned();
    #[cfg(not(windows))]
    let key = Some(SUBPROCESS_CONTROL_ENV.to_owned());
    let marker = key.and_then(|key| environment.shift_remove(&key)).flatten();
    if marker.as_deref() != Some("pipe") {
        return Err(Box::new(ControlChannelError::NotInherited));
    }
    open(ControlChannelOptions {
        fd: SUBPROCESS_CONTROL_FD,
        readable: true,
        writable: true,
        allow_half_open: true,
    })
}
