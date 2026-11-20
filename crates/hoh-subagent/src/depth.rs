//! Delegation-depth accounting.

use hoh_agent::Agent;
use hoh_error::BoxError;

use crate::error::{SubagentError, SubagentErrorCode};

pub fn delegation_depth_of(agent: &dyn Agent) -> u64 {
    // SHIM(hoh-agent): AgentOptions does not yet expose declaration-merged `subagentDepth`.
    // The persisted header remains the source's authoritative monotone floor.
    agent
        .session()
        .header()
        .get("delegationDepth")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

pub fn assert_subagent_max_depth(max_depth: Option<f64>) -> Result<(), BoxError> {
    if max_depth.is_some_and(|value| {
        !value.is_finite()
            || value < 0.0
            || value.is_sign_negative()
            || value.fract() != 0.0
            || value > 9_007_199_254_740_991.0
    }) {
        return Err("subagent maxDepth must be a non-negative safe integer".into());
    }
    Ok(())
}

pub fn assert_child_depth(child_depth: u64, max_depth: Option<u64>) -> Result<(), BoxError> {
    if max_depth.is_some_and(|limit| child_depth > limit) {
        return Err(Box::new(SubagentError::error(
            format!(
                "subagent depth {child_depth} exceeds maxDepth {max_depth}",
                max_depth = max_depth.unwrap_or_default()
            ),
            SubagentErrorCode::DepthExceeded,
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_error::HarnessError;

    #[test]
    fn max_depth_accepts_non_negative_safe_integers_and_absence() {
        for value in [None, Some(0.0), Some(1.0), Some(9_007_199_254_740_991.0)] {
            assert!(assert_subagent_max_depth(value).is_ok());
        }
    }

    #[test]
    fn max_depth_rejects_javascript_invalid_numbers() {
        for value in [
            -0.0,
            -1.0,
            1.5,
            f64::NAN,
            f64::INFINITY,
            9_007_199_254_740_992.0,
        ] {
            assert!(assert_subagent_max_depth(Some(value)).is_err(), "{value:?}");
        }
    }

    #[test]
    fn child_depth_failure_is_machine_routable() {
        let error = assert_child_depth(2, Some(1)).unwrap_err();
        let error = HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.name(), "SubagentError");
        assert_eq!(error.code(), "DEPTH_EXCEEDED");
        assert_eq!(error.message(), "subagent depth 2 exceeds maxDepth 1");
    }
}
