//! Browser-safe durable image selection declaration and pure replay definition.

use crate::{ImageOffloadError, project_message::offload_message_images};
use hoh_error::BoxError;
use hoh_session::{
    MAX_SAFE_INTEGER, SessionEvent, SessionMessageProjection, SessionMessageProjectionContext,
    SessionSeq,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, LazyLock};

/// Exact input-image occurrences selected by one durable offload decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOffloadTarget {
    /// Current message-producing event containing these occurrences.
    pub seq: SessionSeq,
    /// Zero-based depth-first image indexes within the immutable message.
    pub image_indexes: Vec<u64>,
}

/// Data of the `image/offload` session event (`SessionEventMap['image/offload']`).
///
/// Permanently omit selected input-image occurrences from subsequent model requests.
/// Targets name unique current user/message or tool/result nodes. Nonempty, strictly
/// increasing indexes count all images in depth-first order, including nested tool
/// results and already omitted images. Message nodes and identities remain unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageOffloadEvent {
    pub targets: Vec<ImageOffloadTarget>,
}

impl ImageOffloadEvent {
    /// Session event type (`@messageProjection`).
    pub const TYPE: &'static str = "image/offload";
}

/// Whether a durable occurrence index or sequence is canonical: a non-negative safe integer
/// that is not negative zero.
fn index_of(value: &Value) -> Option<u64> {
    if let Some(n) = value.as_u64() {
        return (n <= MAX_SAFE_INTEGER).then_some(n);
    }
    let n = value.as_f64()?;
    (n.is_finite()
        && n.fract() == 0.0
        && n >= 0.0
        && !n.is_sign_negative()
        && n <= MAX_SAFE_INTEGER as f64)
        .then_some(n as u64)
}

const TARGET_SHAPE: &str =
    "image/offload: each target must contain a seq and nonempty imageIndexes";

/// Atomic validation and reconstruction shared by live sessions and detached replay.
pub struct ImageOffloadProjection;

impl SessionMessageProjection for ImageOffloadProjection {
    fn event_type(&self) -> &str {
        ImageOffloadEvent::TYPE
    }

    fn project(
        &self,
        event: &SessionEvent,
        context: SessionMessageProjectionContext<'_>,
    ) -> Result<IndexMap<SessionSeq, Arc<Value>>, BoxError> {
        let targets = event
            .data()
            .as_object()
            .filter(|data| data.len() == 1)
            .and_then(|data| data.get("targets"))
            .and_then(Value::as_array)
            .filter(|targets| !targets.is_empty())
            .ok_or_else(|| {
                ImageOffloadError::boxed(
                    "image/offload: data must contain a nonempty targets array",
                )
            })?;
        let mut messages: IndexMap<SessionSeq, Arc<Value>> = IndexMap::new();
        for target in targets {
            let target = target
                .as_object()
                .filter(|target| target.len() == 2)
                .ok_or_else(|| ImageOffloadError::boxed(TARGET_SHAPE))?;
            let seq = target
                .get("seq")
                .and_then(index_of)
                .ok_or_else(|| ImageOffloadError::boxed(TARGET_SHAPE))?;
            let image_indexes = target
                .get("imageIndexes")
                .and_then(Value::as_array)
                .filter(|indexes| !indexes.is_empty())
                .ok_or_else(|| ImageOffloadError::boxed(TARGET_SHAPE))?;
            // `seq` passed `index_of`, so it is a safe integer.
            let seq_key = SessionSeq::new(seq)?;
            if messages.contains_key(&seq_key) {
                return Err(ImageOffloadError::boxed(format!(
                    "image/offload: duplicate target seq {seq}"
                )));
            }
            if !context.nodes.contains(&seq_key) {
                return Err(ImageOffloadError::boxed(format!(
                    "image/offload: target seq {seq} is not a current surface node"
                )));
            }
            let source = seq
                .checked_sub(context.base_seq.get())
                .and_then(|offset| usize::try_from(offset).ok())
                .and_then(|offset| context.events.get(offset))
                .filter(|source| matches!(source.event_type(), "user/message" | "tool/result"))
                .ok_or_else(|| {
                    ImageOffloadError::boxed(format!(
                        "image/offload: target seq {seq} must be user/message or tool/result"
                    ))
                })?;
            let mut indexes: Vec<u64> = Vec::with_capacity(image_indexes.len());
            for value in image_indexes {
                let index = index_of(value)
                    .filter(|index| indexes.last().is_none_or(|previous| index > previous))
                    .ok_or_else(|| {
                        ImageOffloadError::boxed(
                            "image/offload: imageIndexes must be strictly increasing non-negative safe integers",
                        )
                    })?;
                indexes.push(index);
            }
            let message = match context.messages.get(&seq_key) {
                Some(message) => message.clone(),
                None if source.event_type() == "user/message" => Arc::new(source.data().clone()),
                None => Arc::new(source.data()["message"].clone()),
            };
            messages.insert(
                seq_key,
                Arc::new(offload_message_images(&message, &indexes)?),
            );
        }
        Ok(messages)
    }
}

static IMAGE_OFFLOAD_PROJECTION: LazyLock<Arc<ImageOffloadProjection>> =
    LazyLock::new(|| Arc::new(ImageOffloadProjection));

/// The one immutable definition of the source (`imageOffloadProjection`). A Session compares the
/// identity of its projection definitions, so every caller shares this `Arc`.
pub fn image_offload_projection() -> Arc<dyn SessionMessageProjection> {
    IMAGE_OFFLOAD_PROJECTION.clone()
}
