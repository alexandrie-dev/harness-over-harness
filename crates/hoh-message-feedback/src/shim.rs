// SHIM(hoh-command-feedback): fixed durable feedback category vocabulary;
// replace with the crate's exported FeedbackCategory and FEEDBACK_CATEGORIES.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeedbackCategory {
    TaskResult,
    InstructionFollowing,
    ProductInteraction,
    ServiceStability,
    ResourceCost,
    SecurityPrivacyPermission,
    Other,
}

pub const FEEDBACK_CATEGORIES: [FeedbackCategory; 7] = [
    FeedbackCategory::TaskResult,
    FeedbackCategory::InstructionFollowing,
    FeedbackCategory::ProductInteraction,
    FeedbackCategory::ServiceStability,
    FeedbackCategory::ResourceCost,
    FeedbackCategory::SecurityPrivacyPermission,
    FeedbackCategory::Other,
];
