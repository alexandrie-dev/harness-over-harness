mod common;

use common::*;
use hoh_bash_local::*;
use serde_json::json;

async fn commit(setup: &Setup, config: serde_json::Value) {
    // Every field of the executor Config is volatile: the update commits without remounting.
    assert!(setup.executor_fiber.commit_volatile(config).await.unwrap());
}

#[tokio::test]
async fn uses_changed_budgets_for_later_commands_without_remounting_executor() {
    let setup = setup(json!({"timeoutMs": 60_000})).await;
    let before = setup.executor_fiber.uid();
    commit(
        &setup,
        json!({"timeoutMs": 5_000, "maxOutputBytes": 1_024, "graceMs": 200}),
    )
    .await;
    assert_eq!(setup.executor_fiber.uid(), before);
    let spec = setup.shell.resolve(&request("echo ok"));
    assert_eq!(spec.timeout_ms, 5_000.0);
    assert_eq!(spec.stdout_max_bytes, 1_024);

    // A stored budget the executor cannot use fails the next command, not the profile write.
    commit(
        &setup,
        json!({"timeoutMs": 0, "maxOutputBytes": 1_024, "graceMs": 200}),
    )
    .await;
    assert!(
        resolve_error(&setup.shell, request("echo ok"))
            .contains("timeoutMs must be a positive finite number")
    );

    commit(
        &setup,
        json!({"timeoutMs": 5_000, "maxOutputBytes": 1_024, "graceMs": 9_007_199_254_740_991_u64}),
    )
    .await;
    assert!(resolve_error(&setup.shell, request("echo ok")).contains("graceMs must be no greater"));

    commit(
        &setup,
        json!({"timeoutMs": 5_000, "maxOutputBytes": 1_024, "graceMs": 200}),
    )
    .await;
    assert_eq!(setup.shell.resolve(&request("echo ok")).timeout_ms, 5_000.0);
    // `live.replace({})`: the defaults come back.
    commit(&setup, json!({})).await;
    assert_eq!(
        setup.shell.resolve(&request("echo ok")).timeout_ms,
        120_000.0
    );
    assert_eq!(setup.executor_fiber.uid(), before);
}

#[test]
fn direct_config_validation_preserves_the_nan_case() {
    let config = Config {
        timeout_ms: hoh_kernel::Volatile(f64::NAN),
        ..Config::default()
    };
    let error = assert_serviceable_bash_config(&config).unwrap_err();
    assert!(error.to_string().contains("timeoutMs"));
}
