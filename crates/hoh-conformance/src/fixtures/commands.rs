//! `snapshots/sdk/command-identity/runtime.mjs`: rename the native permission definition and shadow
//! its old name for one SDK Session.
use super::SNAPSHOTS;
use hoh_agent::AgentCreated;
use hoh_commands::{CommandDefinition, Commands};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_loader::PluginRegistry;

#[derive(Inject)]
pub struct CommandsDeps {
    commands: Service<Commands>,
}

pub struct SnapshotCommandIdentity;
impl Plugin for SnapshotCommandIdentity {
    const NAME: &'static str = "snapshot-command-identity";
    type Config = ();
    type Deps = CommandsDeps;
    async fn apply(ctx: Context, deps: CommandsDeps, _: Cfg<()>) -> Result<(), BoxError> {
        let commands = deps.commands;
        ctx.listen()
            .on_serial::<AgentCreated, (), _>(move |event| {
                let commands = commands.clone();
                async move {
                    let agent = &event.agent;
                    let Some(permission) = commands.find(agent, "permission") else {
                        return Err("Snapshot requires the native permission definition".into());
                    };
                    // The two registrations live as long as the Agent's own context
                    // (`agent.ctx.plugin({ name: 'snapshot-scoped-command-identity', … })`).
                    commands.register(
                        agent.ctx(),
                        CommandDefinition {
                            name: "access".into(),
                            ..permission
                        },
                    )?;
                    let mut unrelated = CommandDefinition::new(
                        "permission",
                        "Unrelated same-name command",
                        |_| async { Err("SDK must not execute the unrelated command".into()) },
                    );
                    unrelated.definition_id = Some("snapshot/unrelated-permission".into());
                    commands.register(agent.ctx(), unrelated)?;
                    let active = commands
                        .find(agent, "permission")
                        .and_then(|found| found.definition_id);
                    if active.as_ref().map(|id| id.as_str())
                        != Some("snapshot/unrelated-permission")
                    {
                        return Err("Snapshot command shadow did not become active".into());
                    }
                    Ok(None)
                }
            })?;
        Ok(())
    }
}

pub fn add(registry: &mut PluginRegistry) {
    registry
        .add::<SnapshotCommandIdentity>(&format!("{SNAPSHOTS}sdk/command-identity/runtime.mjs"));
}
