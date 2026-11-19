//! Replay-aware token measurement for language-model requests and responses.

pub mod breakdown_projection;
pub mod estimate;
pub mod measurement_projection;
pub mod route_pricing;
pub mod service;
pub mod surface_fold;
pub mod surface_projection;
pub mod turn_usage;
pub mod types;
pub mod usage_projection;

pub use breakdown_projection::context_breakdown_projection_definition;
pub use estimate::{
    ROLE_OVERHEAD, estimate_content, estimate_message, estimate_structural_block,
    estimate_system_message, estimate_tools_tokens,
};
pub use measurement_projection::{
    MeasurementAnchor, MeasurementState, StepStart, advance_measurement,
    measurement_projection_definition,
};
pub use route_pricing::{PricedSurface, price_surface};
pub use service::{Deps, TokenMeter, TokenMeterPlugin, TokenMeterRuntime};
pub use surface_fold::{
    MeterSurfaceNode, SurfaceTokenPlan, SurfaceTokenTarget, commit_surface_tokens,
    plan_surface_tokens,
};
pub use surface_projection::{ShadowPriceClaim, SurfaceTokensFold, fold_surface_projection};
pub use turn_usage::{TurnTokenUsage, TurnTokenUsageRoute, derive_turn_token_usage};
pub use types::*;
pub use usage_projection::{
    ContextPressureState, context_pressure_projection_definition, token_usage_projection_definition,
};

/// Browser-safe projection contracts and pure turn fold (`./client` in the source package).
pub mod client {
    pub use crate::{
        ContextBreakdownProjection, ContextPressureProjection, TokenUsageProjection,
        TurnTokenUsage, TurnTokenUsageRoute, derive_turn_token_usage,
    };
}

use hoh_error::BoxError;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct TokenMeterError(String);

fn error(message: impl Into<String>) -> BoxError {
    Box::new(TokenMeterError(message.into()))
}

fn normalize_nonnegative_safe_integers(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                normalize_nonnegative_safe_integers(value);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                normalize_nonnegative_safe_integers(value);
            }
        }
        serde_json::Value::Number(number) if number.is_f64() => {
            if let Some(number) = number.as_f64()
                && number.is_finite()
                && number >= 0.0
                && !number.is_sign_negative()
                && number.fract() == 0.0
                && number <= hoh_session::MAX_SAFE_INTEGER as f64
            {
                *value = serde_json::Value::from(number as u64);
            }
        }
        _ => {}
    }
}
