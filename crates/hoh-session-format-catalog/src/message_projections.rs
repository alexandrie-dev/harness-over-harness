use hoh_compaction_image_offload::image_offload_projection;
use hoh_session::MessageProjections;
use std::sync::{Arc, Mutex};
/// Installed detached replay interpreters; recovery listeners are not mounted here.
pub fn current_session_message_projections() -> MessageProjections {
    Arc::new(Mutex::new(vec![image_offload_projection()]))
}
