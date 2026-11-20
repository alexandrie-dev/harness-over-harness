//! `command-identity` runtime fixture: the Agent sees `access` (the native permission definition)
//! and an unrelated `permission`; the root registry is untouched.
use hoh_agent::{Agents, CreateAgentOptions};
use hoh_agent_loop_testkit::{
    AgentLoopTestDependenciesOptions, mount_agent_loop_test_dependencies,
    mount_agent_loop_test_harness,
};
use hoh_commands::{CommandDefinition, CommandResult, CommandRuntime, Commands};
use hoh_conformance::fixtures::commands::SnapshotCommandIdentity;
use hoh_kernel::Kernel;
use hoh_session::SessionId;
use serde_json::Value;

async fn setup(native: bool) -> (Kernel, Result<(), String>) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount_agent_loop_test_dependencies(&ctx, AgentLoopTestDependenciesOptions::default())
        .await
        .unwrap();
    ctx.plugin::<CommandRuntime>(Value::Null)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let commands = ctx.get::<Commands>().unwrap();
    if native {
        let mut native = CommandDefinition::new("permission", "native", |_| async {
            Ok(CommandResult::success(None::<String>))
        });
        native.definition_id = Some("native/permission".into());
        std::mem::forget(commands.register(&ctx, native).unwrap());
    }
    ctx.plugin::<SnapshotCommandIdentity>(Value::Null)
        .unwrap()
        .settled()
        .await
        .unwrap();
    mount_agent_loop_test_harness(&ctx).await.unwrap();
    let agents = ctx.get::<Agents>().unwrap();
    let created = agents
        .create(&ctx, CreateAgentOptions::new(SessionId::from("identity")))
        .await;
    let result = match created {
        Ok(created) => {
            let agent = created.agent;
            let id = |name: &str| {
                commands
                    .find(&agent, name)
                    .and_then(|d| d.definition_id)
                    .map(|id| id.as_str().to_owned())
            };
            assert_eq!(id("access").as_deref(), Some("native/permission"));
            assert_eq!(
                id("permission").as_deref(),
                Some("snapshot/unrelated-permission")
            );
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    };
    (kernel, result)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_native_definition_is_renamed_and_its_old_name_is_shadowed_for_the_agent() {
    let (kernel, result) = setup(true).await;
    result.unwrap();
    kernel.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_native_definition_fails_agent_creation() {
    let (kernel, result) = setup(false).await;
    assert!(result.unwrap_err().contains("native permission definition"),);
    kernel.shutdown().await;
}
