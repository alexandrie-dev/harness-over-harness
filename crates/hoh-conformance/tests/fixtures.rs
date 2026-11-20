use hoh_conformance::fixtures;
use hoh_loader::PluginRegistry;
use std::path::Path;

const PORTED: &[&str] = &[
    "compaction-working-directory",
    "parent-agent-preset",
    "parent-sandbox-override",
    "in-history-prompt-update",
    "session-write-grant",
    "child-question-tripwire",
    "subagent-activation-limit",
    "subagent-settlement-barrier",
];

#[test]
fn every_ported_fixture_is_registered_under_the_local_name_of_its_typescript_file() {
    let mut registry = PluginRegistry::default();
    fixtures::add(&mut registry);
    let clone = Path::new("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness");
    for name in PORTED {
        let relative = format!("packages/test-support/session-snapshot/tests/fixtures/{name}.ts");
        assert!(
            clone.join(&relative).is_file(),
            "{relative} is not a dsh fixture"
        );
        assert!(
            registry.resolve(&format!("local:{relative}")).is_some(),
            "{name} is not registered under its local: name"
        );
    }
}

/// The fixtures of a scenario directory (`./name.mjs`), as `scenario dir -> file`; registered under
/// `local:snapshots/<dir>/<file>`.
const PORTED_IN_SCENARIOS: &[(&str, &str)] = &[
    ("sdk/manual-compact", "capture-requests.mjs"),
    ("sdk/command-identity", "runtime.mjs"),
    ("sdk/text-turn", "feedback-producer.mjs"),
    ("session/deepseek-fetch", "deepseek-fetch-fixture.mjs"),
    ("session/model-switch-compaction", "driver.mjs"),
    ("session/model-switch-notice", "model-switch-driver.mjs"),
    ("session/multimodal-spill-ends", "fixture.mjs"),
    ("session/subagent-parallel", "execution-order.mjs"),
    ("session/subagent-spawn-in-process", "settlement-order.mjs"),
    ("session/web-fetch", "web-fetch-fixture-server.mjs"),
    (
        "session/web-fetch-transport-failure",
        "web-fetch-transport-failure-fixture.mjs",
    ),
];

#[test]
fn every_ported_scenario_fixture_is_registered_under_its_snapshots_path() {
    let mut registry = PluginRegistry::default();
    fixtures::add(&mut registry);
    let clone = Path::new("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness");
    for (dir, file) in PORTED_IN_SCENARIOS {
        let relative = format!("snapshots/{dir}/{file}");
        assert!(
            clone.join(&relative).is_file(),
            "{relative} is not a dsh fixture"
        );
        assert!(
            registry.resolve(&format!("local:{relative}")).is_some(),
            "{relative} is not registered"
        );
    }
}

#[test]
fn unported_names_the_local_plugins_without_a_counterpart_in_any_group_row() {
    let patch = serde_json::json!([{"insert": [
        {"id": "a", "name": "@deepseek-ai/dsh-llm-replay"},
        {"id": "b", "name": "local:snapshots/sdk/desktop-context/clock.mjs"},
        {"id": "c", "name": "local:snapshots/session/web-fetch/web-fetch-fixture-server.mjs"},
        {"id": "d", "group": true, "config": [
            {"id": "e", "name": "local:packages/core/tools/tests/fixtures/literal-sdk.ts"},
            {"id": "f", "name": "local:snapshots/sdk/desktop-context/clock.mjs"}
        ]}
    ]}])
    .to_string();
    let names = fixtures::unported(&patch);
    assert_eq!(
        names,
        [
            "local:snapshots/sdk/desktop-context/clock.mjs",
            "local:packages/core/tools/tests/fixtures/literal-sdk.ts"
        ]
    );
    assert_eq!(
        fixtures::unported_message(&names),
        "фикстура clock.mjs не перенесена (local:snapshots/sdk/desktop-context/clock.mjs); \
         фикстура literal-sdk.ts не перенесена (local:packages/core/tools/tests/fixtures/literal-sdk.ts)"
    );
    assert!(fixtures::unported("not json").is_empty());
}

#[test]
fn an_unported_fixture_stays_unresolved() {
    let mut registry = PluginRegistry::default();
    fixtures::add(&mut registry);
    assert!(
        registry
            .resolve("local:packages/test-support/session-snapshot/tests/fixtures/subagent-durability-failure.ts")
            .is_none()
    );
}

/// Review 213901 №4: the grant of `session-write-grant` is gone as soon as the releasing event
/// handler returns — `dispose()` of the source is synchronous, a request may follow at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_released_write_grant_is_gone_before_the_next_request() {
    use hoh_kernel::Kernel;
    use hoh_sandbox_policy::{SandboxPolicy, SandboxPolicyRequest, SandboxPolicyService};
    use hoh_session::{Session, SessionId};
    use serde_json::json;
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let load = |config: serde_json::Value| {
        let fiber = ctx.plugin::<SandboxPolicyService>(config).unwrap();
        async move { fiber.settled().await.unwrap() }
    };
    let projection = ctx
        .plugin::<hoh_session_projection::SessionProjectionRegistry>(json!({}))
        .unwrap();
    projection.settled().await.unwrap();
    load(json!({"mode":"workspace-write"})).await;
    let policy = ctx.get::<SandboxPolicy>().unwrap();
    let id = SessionId::from("grant-loop");
    let header =
        json!({"version": 4, "id": id, "createdAt": 0, "isSeeded": false, "cwd": "/workspace"});
    let session = Session::create(id, None, Some(header), None, None).unwrap();
    let roots = || {
        policy
            .resolve(SandboxPolicyRequest {
                session: Some(&session),
                mode: None,
            })
            .unwrap()
            .additional_writable_roots
            .map(|roots| roots.to_vec())
            .unwrap_or_default()
    };
    for round in 0..200 {
        let disposer = policy
            .register_writable_root(&ctx, &session, "/workspace/extra")
            .unwrap();
        assert_eq!(roots(), ["/workspace/extra"], "round {round}");
        // The release is called from a handler that does not await; yielding to other workers
        // between the call and the check must not matter.
        fixtures::release_grant(disposer);
        assert!(
            roots().is_empty(),
            "grant survived the release in round {round}"
        );
    }
    kernel.shutdown().await;
}
