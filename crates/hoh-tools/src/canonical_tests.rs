//! White-box tests of the canonical mark: only a result frozen by `marked` ignores its setters; a result
//! the registry merely marks as canonical (`markCanonical` of the source) stays an editable plain object.

use hoh_timeout::AbortController;
use serde_json::{Value, json};

use crate::{
    ToolExecution, ToolExecutionFailure, ToolExecutionInput, ToolExecutionResult,
    ToolExecutionSuccess, ToolFailure,
};

fn execution() -> ToolExecution {
    let controller = AbortController::new();
    let input = ToolExecutionInput::new("call".into(), "tool", json!({}), controller.signal());
    ToolExecution::mint(&input, json!({}), None, None)
}

fn failure() -> ToolExecutionResult {
    ToolExecutionResult::Failure(ToolExecutionFailure::new(
        ToolFailure {
            message: "original".into(),
            info: None,
        },
        vec![],
    ))
}

#[test]
fn a_normalized_failure_stays_editable_until_final_materialization() {
    let exec = execution();
    let mut result = failure().marked_editable(&exec);
    assert!(result.seal().is_some_and(|seal| seal.token == exec.token));
    let ToolExecutionResult::Failure(edited) = &mut result else {
        unreachable!()
    };
    edited.set_meta(Some(json!({ "edited": true })));
    edited.set_error(ToolFailure {
        message: "changed".into(),
        info: None,
    });
    assert_eq!(result.meta(), Some(&json!({ "edited": true })));
    assert_eq!(
        result.error().map(|error| error.message.as_str()),
        Some("changed")
    );
}

#[test]
fn a_frozen_result_keeps_its_original_and_its_mark() {
    let exec = execution();
    let mut result =
        ToolExecutionResult::Success(ToolExecutionSuccess::new(json!("original"), vec![]))
            .marked(&exec);
    let ToolExecutionResult::Success(frozen) = &mut result else {
        unreachable!()
    };
    frozen.set_value(json!(42));
    frozen.set_meta(Some(Value::Null));
    assert_eq!(result.value(), Some(&json!("original")));
    assert_eq!(result.meta(), None);
    // A copy is a new object: unmarked and editable.
    let mut copy = result.clone();
    assert!(copy.seal().is_none());
    let ToolExecutionResult::Success(copy_success) = &mut copy else {
        unreachable!()
    };
    copy_success.set_value(json!(42));
    assert_eq!(copy.value(), Some(&json!(42)));
}
