mod support;
use hoh_kernel::{Cfg, Context, Error, Kernel, Plugin, Realm};
use hoh_loader::startup_diagnostics::*;
use hoh_loader::*;
use std::sync::Arc;
use support::{live::*, *};
#[derive(Default)]
struct ProviderGate {
    registered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
hoh_kernel::service!(ProviderGateKey: ProviderGate = "providerGate");
hoh_kernel::service!(ObservedAgents: () = "agents");
struct HeldProvider;
impl Plugin for HeldProvider {
    const NAME: &'static str = "held-provider";
    type Config = serde_json::Value;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<Self::Config>) -> Result<(), Error> {
        let gate = ctx.get::<ProviderGateKey>().unwrap();
        ctx.provide::<ObservedAgents>(Arc::new(())).await?;
        gate.registered.notify_one();
        gate.release.notified().await;
        Ok(())
    }
}
struct NestedProvider;
impl Plugin for NestedProvider {
    const NAME: &'static str = "nested-provider";
    type Config = serde_json::Value;
    type Deps = ();
    async fn apply(ctx: Context, _: (), cfg: Cfg<Self::Config>) -> Result<(), Error> {
        let owner = if cfg.get()["other"] == true {
            ctx.isolate("agents", Realm::new())
        } else {
            ctx
        };
        owner.plugin::<HeldProvider>(serde_json::json!({}))?;
        Ok(())
    }
}
#[tokio::test(start_paused = true)]
async fn reports_a_registered_same_scope_provider_whose_value_is_unavailable() {
    let world = World::registry(
        "- {id: consumer, name: noop, inject: [agents, unprovided]}",
        |registry| {
            registry.add::<HeldProvider>("held-provider");
        },
    )
    .await;
    let gate = Arc::new(ProviderGate::default());
    world
        .ctx
        .provide::<ProviderGateKey>(gate.clone())
        .await
        .unwrap();
    world
        .loader
        .create(
            entries("- {id: agent, name: held-provider}").remove(0),
            Some("include"),
        )
        .await
        .unwrap();
    gate.registered.notified().await;
    let report = capture_startup_composition(&world.ctx).await.unwrap();
    let service = report
        .services
        .iter()
        .find(|service| service.service == "agents")
        .unwrap();
    assert_eq!(service.providers.len(), 1);
    assert_eq!(service.providers[0].entry, "include:agent");
    assert_eq!(service.providers[0].visibility, "same-scope");
    // A typed Arc is unavailable to other fibers while its provider is Loading (R26).
    assert_eq!(service.providers[0].state, "loading");
    assert!(
        report
            .services
            .iter()
            .any(|service| service.service == "unprovided" && service.providers.is_empty())
    );
    gate.release.notify_one();
    world.loader.settled().await;
    assert_eq!(
        world.row("consumer").fiber.unwrap().state(),
        hoh_kernel::FiberState::Pending
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn attributes_a_nested_provider_to_its_loader_entry() {
    for other in [false, true] {
        let world = World::registry(
            "- {id: consumer, name: noop, inject: [agents, unprovided]}",
            |registry| {
                registry.add::<NestedProvider>("nested-provider");
            },
        )
        .await;
        let gate = Arc::new(ProviderGate::default());
        world
            .ctx
            .provide::<ProviderGateKey>(gate.clone())
            .await
            .unwrap();
        let mut entry = entries("- {id: agent, name: nested-provider}").remove(0);
        entry.config = serde_json::json!({"other":other});
        world.loader.create(entry, Some("include")).await.unwrap();
        gate.registered.notified().await;
        let report = capture_startup_composition(&world.ctx).await.unwrap();
        let service = report
            .services
            .iter()
            .find(|service| service.service == "agents")
            .unwrap();
        assert_eq!(service.providers.len(), 1);
        assert_eq!(service.providers[0].entry, "include:agent");
        assert_eq!(service.providers[0].state, "loading");
        assert_eq!(
            service.providers[0].visibility,
            if other { "other-scope" } else { "same-scope" }
        );
        gate.release.notify_one();
        world.loader.settled().await;
        assert_eq!(
            world.row("consumer").fiber.unwrap().state(),
            hoh_kernel::FiberState::Pending
        );
        world.close().await;
    }
}
async fn failure(text: &str) -> (Kernel, LoadError) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cordis.yml");
    std::fs::write(&path, text).unwrap();
    let kernel = Kernel::start();
    let mut registry = PluginRegistry::default();
    registry.add::<Noop>("noop");
    let error = boot(
        &kernel.root(),
        BootOptions {
            registry: Arc::new(registry),
            base: path,
            layers: vec![],
            env: env(dir.path()),
        },
    )
    .await
    .err()
    .unwrap();
    (kernel, error)
}
#[tokio::test(start_paused = true)]
async fn retains_disabled_and_waiting_entries_after_the_real_loader_has_been_disposed() {
    let (kernel,error)=failure("- {id: agent, name: noop, disabled: true, config: {privateValue: do-not-export-this}}\n- {id: agent-loop, name: noop, inject: [agents]}\n").await;
    let LoadError::Startup(error) = error else {
        panic!("expected startup error")
    };
    assert!(kernel.root().get::<LoaderKey>().is_none());
    assert!(
        error
            .composition
            .plugins
            .iter()
            .any(|p| p.module == "noop" && p.state == "disabled" && p.version == "unknown")
    );
    assert!(
        error
            .composition
            .plugins
            .iter()
            .any(|p| p.state == "pending" && p.missing == ["agents"])
    );
    assert!(error.composition.services[0].providers.is_empty());
    assert!(
        !serde_json::to_string(&error.composition)
            .unwrap()
            .contains("do-not-export-this")
    );
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn preserves_the_startup_failure_when_the_loader_is_already_unavailable_to_diagnostic_capture()
 {
    let (kernel, error) = failure("- {id: agent-loop, name: noop, inject: [agents]}\n").await;
    let message = error.to_string();
    assert!(capture_startup_composition(&kernel.root()).await.is_err());
    assert_eq!(error.to_string(), message);
    assert!(message.contains("hoh: startup failed: 1 required plugin did not activate"));
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn identifies_a_registered_provider_that_is_isolated_from_the_waiting_consumer() {
    hoh_kernel::service!(Agents: () = "agents");
    let world = World::new("- {id: consumer, name: noop, inject: [agents]}\n").await;
    world
        .ctx
        .isolate("agents", Realm::new())
        .provide::<Agents>(Arc::new(()))
        .await
        .unwrap();
    let report = capture_startup_composition(&world.ctx).await.unwrap();
    assert_eq!(report.services.len(), 1);
    assert_eq!(report.services[0].providers.len(), 1);
    let provider = &report.services[0].providers[0];
    assert_eq!(provider.entry, "unknown");
    assert_eq!(provider.state, "active");
    assert_eq!(provider.visibility, "other-scope");
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn reports_omitted_plugin_and_missing_service_counts_instead_of_silently_dropping_rows() {
    let names = (0..257).map(|i| format!("missing{i}")).collect::<Vec<_>>();
    let mut text = format!(
        "- {{id: agent-loop, name: noop, inject: [{}]}}\n",
        names.join(", ")
    );
    for i in 0..257 {
        text.push_str(&format!(
            "- {{id: disabled-{i}, name: noop, disabled: true}}\n"
        ));
    }
    let (kernel, error) = failure(&text).await;
    let LoadError::Startup(error) = error else {
        panic!("expected startup error")
    };
    assert_eq!(error.composition.plugins.len(), 256);
    assert!(error.composition.omitted_plugins >= 2);
    assert_eq!(error.composition.services.len(), 256);
    assert_eq!(error.composition.omitted_services, 1);
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn retains_the_loader_disablement_after_disposal_and_reports_missing_source_metadata() {
    let world = World::new("- {id: agent, name: noop}\n").await;
    world.row("agent").fiber.unwrap().dispose().await;
    world.loader.settled().await;
    let report = capture_startup_composition(&world.ctx).await.unwrap();
    assert!(
        report
            .plugins
            .iter()
            .any(|p| p.id == "include:agent" && p.state == "disabled")
    );
    assert!(world.row("agent").disabled);
    world.close().await;
}
#[test]
fn exports_the_named_types_used_by_startup_composition_diagnostics() {
    fn plugin(report: &StartupCompositionDiagnostic) -> Option<&StartupPluginState> {
        report.plugins.first()
    }
    fn service(report: &StartupCompositionDiagnostic) -> Option<&StartupServiceState> {
        report.services.first()
    }
    let report = StartupCompositionDiagnostic {
        plugins: vec![],
        services: vec![],
        omitted_plugins: 0,
        omitted_services: 0,
    };
    assert!(plugin(&report).is_none());
    assert!(service(&report).is_none());
}
