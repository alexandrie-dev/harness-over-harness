//! Port of @deepseek-ai/dsh-compaction-image-offload (packages/compaction/compaction-image-offload).
//!
//! Image offload executor for the compaction seam. When an image-capable route
//! fails a request with `IMAGE_OFFLOAD_REQUIRED`, the plugin records one
//! `image/offload` decision selecting the oldest retained input occurrences
//! and retries through the agent or compaction summary error waterfall. Every route
//! sends placeholder text for those occurrences in subsequent requests.

mod image_offload;
mod project_message;
pub mod projection;
pub mod shim;

pub use image_offload::offload_oldest_images;
pub use project_message::offload_message_images;
pub use projection::{
    ImageOffloadEvent, ImageOffloadProjection, ImageOffloadTarget, image_offload_projection,
};

use hoh_agent::{AgentRequestError, Agents, RequestErrorAction};
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Plugin, Service};
use hoh_llm::{IMAGE_OFFLOAD_REQUIRED_CODE, LlmError, LlmFailure};
use hoh_session::Sessions;
use shim::CompactionSummaryError;

/// Plain `Error` of the source (`new Error('image/offload: ...')`): a message and nothing else.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ImageOffloadError(String);

impl ImageOffloadError {
    pub(crate) fn boxed(message: impl Into<String>) -> BoxError {
        Box::new(Self(message.into()))
    }
}

/// `export const inject = ['agents', 'sessions']`.
#[derive(Inject)]
pub struct Deps {
    _agents: Service<Agents>,
    sessions: Service<Sessions>,
}

/// `export const name`/`inject`/`apply` of the source: one plugin, no configuration.
pub struct ImageOffloadPlugin;

impl Plugin for ImageOffloadPlugin {
    const NAME: &'static str = "compaction-image-offload";
    type Config = NoConfig;
    type Deps = Deps;

    /// Mount agent and summary recovery listeners without configuration.
    async fn apply(ctx: Context, deps: Deps, _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        deps.sessions
            .register_message_projection(&ctx, image_offload_projection())?;
        ctx.on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(
            |event, next| async move {
                let failure = &event.failure;
                if failure.code != IMAGE_OFFLOAD_REQUIRED_CODE || event.signal.aborted() {
                    return next.run().await;
                }
                let Some(count) = failure.offload_images else {
                    return next.run().await;
                };
                // A durable surface repair, not a provider retry: it spends no retry budget and
                // logs no retry event.
                let session = event.agent.session();
                let nodes = session.surface()?.nodes()?;
                if !offload_oldest_images(session, &nodes, count)? {
                    return next.run().await;
                }
                Ok(Some(RequestErrorAction::Retry))
            },
        )?;
        ctx.on_waterfall::<CompactionSummaryError, bool, _>(|event, next| async move {
            let count = HarnessError::of(&event.error)
                .filter(|error| {
                    error.is_named(LlmError::NAME) && error.code() == IMAGE_OFFLOAD_REQUIRED_CODE
                })
                .and_then(|error| error.detail::<LlmFailure>())
                .and_then(|failure| failure.offload_images);
            let Some(count) = count else {
                return next.run().await;
            };
            if let Some(signal) = &event.signal {
                signal
                    .throw_if_aborted()
                    .map_err(|reason| -> BoxError { Box::new(reason) })?;
            }
            if !offload_oldest_images(&event.session, &event.source_event_seqs, count)? {
                return next.run().await;
            }
            Ok(true)
        })?;
        Ok(())
    }
}
