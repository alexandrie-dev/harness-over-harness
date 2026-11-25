//! Window inventory of linked plugins and their current loader state.
use hoh_kernel::{Cfg, Context, FiberState, Inject, NoConfig, Plugin, Service};
use hoh_loader::{EntryStatus, LoaderKey};
use hoh_protocol::{PluginInventoryItem, PluginListParams};
use hoh_sdk_jsonrpc_server::HohRpcMethods;
use hoh_session::{SessionId, Sessions};
use serde_json::Value;

#[derive(Inject)]
pub struct Deps {
    loader: Service<LoaderKey>,
    methods: Service<HohRpcMethods>,
    sessions: Service<Sessions>,
}
pub struct PluginInventory;
impl Plugin for PluginInventory {
    const NAME: &'static str = "hoh:plugin-inventory";
    type Config = NoConfig;
    type Deps = Deps;
    async fn apply(ctx: Context, deps: Deps, _: Cfg<NoConfig>) -> Result<(), hoh_kernel::Error> {
        deps.methods
            .register(&ctx, "hoh/plugins/list", move |params| {
                let loader = deps.loader.clone();
                let sessions = deps.sessions.clone();
                async move {
                    let params: PluginListParams = serde_json::from_value(params)?;
                    let session = params
                        .session_id
                        .as_ref()
                        .and_then(|id| sessions.get(&SessionId::from(id.clone())));
                    let mut plugins = Vec::new();
                    for (id, text) in hoh_authored_plugins::PLUGINS {
                        let manifest: Value = serde_json::from_str(text)?;
                        let address = format!("include:author-{id}");
                        let entry = loader.resolve(&address);
                        let (status, diagnostic) = match (loader.diagnostic_kind(&address), entry) {
                            (Some("configuration-error"), _) => (
                                "configuration-error",
                                Some("Plugin configuration or resolution failed".into()),
                            ),
                            (Some(_), _) => {
                                ("apply-error", Some("Plugin failed to activate".into()))
                            }
                            (None, None) => {
                                ("unmounted", Some("Not present in this composition".into()))
                            }
                            (None, Some(entry)) if entry.disabled => ("disabled", None),
                            (None, Some(entry)) if entry.status == EntryStatus::Unresolved => (
                                "configuration-error",
                                Some("Plugin configuration or resolution failed".into()),
                            ),
                            (None, Some(entry)) => match entry.fiber {
                                Some(fiber) if fiber.state() == FiberState::Active => {
                                    ("mounted", None)
                                }
                                Some(fiber) if fiber.state() == FiberState::Failed => {
                                    ("apply-error", Some("Plugin failed to activate".into()))
                                }
                                _ => ("pending", Some("Plugin is waiting for its services".into())),
                            },
                        };
                        let record = manifest
                            .get("panel")
                            .and_then(|v| v.get("record"))
                            .and_then(Value::as_str)
                            .map(|record| format!("plugin:{id}/{record}"));
                        let data = session.as_ref().and_then(|session| {
                            record.as_ref().and_then(|record| {
                                session
                                    .snapshot_events(None, None)
                                    .iter()
                                    .rev()
                                    .find(|event| event.event_type() == record)
                                    .map(|event| event.data().clone())
                            })
                        });
                        let diagnostic = diagnostic.or_else(|| {
                            (record.is_some() && params.session_id.is_some() && session.is_none())
                                .then(|| "Conversation is not loaded".into())
                        });
                        plugins.push(PluginInventoryItem {
                            id: (*id).into(),
                            manifest,
                            status: status.into(),
                            diagnostic,
                            data,
                        });
                    }
                    Ok(serde_json::to_value(plugins)?)
                }
            })?;
        Ok(())
    }
}
