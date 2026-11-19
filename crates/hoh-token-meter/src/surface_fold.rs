use crate::{error, estimate_message, estimate_structural_block};
use hoh_attachment::FileAttachmentRef;
use hoh_error::BoxError;
use hoh_llm::{ImageBlock, Message};
use hoh_session::{SessionEvent, SessionSeq, SurfaceOp, derive_event_message};
use serde::{Deserialize, Serialize};

/// One retained surface node with the attachment occurrences route pricing replaces.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeterSurfaceNode {
    pub seq: SessionSeq,
    pub heuristic_tokens: u64,
    pub image_structural_tokens: u64,
    pub file_structural_tokens: u64,
    pub images: Vec<ImageBlock>,
    pub files: Vec<FileAttachmentRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceTokenTarget {
    Append,
    Replace { start_idx: usize, end_idx: usize },
}

/// A validated surface transition which has not mutated the surface yet.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceTokenPlan {
    pub tokens: u64,
    pub delta_tokens: i64,
    pub node: MeterSurfaceNode,
    pub target: SurfaceTokenTarget,
}

fn analyze_node(event: &SessionEvent) -> Result<MeterSurfaceNode, BoxError> {
    let Some(raw) = derive_event_message(event, None) else {
        return Ok(MeterSurfaceNode {
            seq: event.seq(),
            heuristic_tokens: 0,
            image_structural_tokens: 0,
            file_structural_tokens: 0,
            images: vec![],
            files: vec![],
        });
    };
    let message: Message = serde_json::from_value((*raw).clone())?;
    let heuristic_tokens = estimate_message(&message);
    let mut images = vec![];
    let mut files = vec![];
    let mut image_structural_tokens = 0_u64;
    let mut file_structural_tokens = 0_u64;
    for block in message.content() {
        if let Some(image) = block.image() {
            images.push(image);
            image_structural_tokens =
                image_structural_tokens.saturating_add(estimate_structural_block(block));
        } else if let Some(file) = block.file() {
            files.push(file.attachment);
            file_structural_tokens =
                file_structural_tokens.saturating_add(estimate_structural_block(block));
        }
    }
    Ok(MeterSurfaceNode {
        seq: event.seq(),
        heuristic_tokens,
        image_structural_tokens,
        file_structural_tokens,
        images,
        files,
    })
}

fn signed_delta(next: u64, removed: u64) -> Result<i64, BoxError> {
    i64::try_from(i128::from(next) - i128::from(removed))
        .map_err(|_| error("token surface: signed token delta is out of range"))
}

/// Validate and price one surface event without mutating the surface.
pub fn plan_surface_tokens(
    nodes: &[MeterSurfaceNode],
    event: &SessionEvent,
) -> Result<SurfaceTokenPlan, BoxError> {
    let node = analyze_node(event)?;
    let tokens = node.heuristic_tokens;
    let op: SurfaceOp = serde_json::from_value(
        event
            .surface_op()
            .cloned()
            .ok_or_else(|| error("token surface: event is missing surfaceOp"))?,
    )?;
    match op {
        SurfaceOp::Append(_) => Ok(SurfaceTokenPlan {
            tokens,
            delta_tokens: i64::try_from(tokens)
                .map_err(|_| error("token surface: token count is out of range"))?,
            node,
            target: SurfaceTokenTarget::Append,
        }),
        SurfaceOp::Replace(replace) => {
            let start_idx = nodes.iter().position(|node| node.seq == replace.start_seq);
            let end_idx = nodes.iter().position(|node| node.seq == replace.end_seq);
            let (Some(start_idx), Some(end_idx)) = (start_idx, end_idx) else {
                return Err(error(format!(
                    "token surface: replace at seq {} has invalid current range {}-{}",
                    event.seq().get(),
                    replace.start_seq.get(),
                    replace.end_seq.get()
                )));
            };
            if start_idx > end_idx {
                return Err(error(format!(
                    "token surface: replace at seq {} has invalid current range {}-{}",
                    event.seq().get(),
                    replace.start_seq.get(),
                    replace.end_seq.get()
                )));
            }
            let removed = nodes[start_idx..=end_idx]
                .iter()
                .fold(0_u64, |sum, node| sum.saturating_add(node.heuristic_tokens));
            Ok(SurfaceTokenPlan {
                tokens,
                delta_tokens: signed_delta(tokens, removed)?,
                node,
                target: SurfaceTokenTarget::Replace { start_idx, end_idx },
            })
        }
    }
}

/// Apply one validated plan to the exact surface it was built against.
pub fn commit_surface_tokens(nodes: &mut Vec<MeterSurfaceNode>, plan: SurfaceTokenPlan) {
    match plan.target {
        SurfaceTokenTarget::Append => nodes.push(plan.node),
        SurfaceTokenTarget::Replace { start_idx, end_idx } => {
            nodes.splice(start_idx..=end_idx, [plan.node]);
        }
    }
}
