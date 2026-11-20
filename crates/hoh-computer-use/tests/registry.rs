use hoh_computer_use::{
    ComputerUse, ComputerUseRegistry,
    brand::{ComputerUseProviderName, computer_use_provider_name},
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Inject, Kernel, Plugin, Service, ServiceKey};
use std::sync::{Arc, Barrier};

fn mcp() -> ComputerUseProviderName {
    computer_use_provider_name("cua-driver-mcp")
}

fn native() -> ComputerUseProviderName {
    computer_use_provider_name("cua-driver-native")
}

#[derive(Inject)]
pub struct ProviderDeps {
    computer_use: Service<ComputerUse>,
}

#[hoh_kernel::plugin(TestComputerUseProvider, name = "test-computer-use-provider")]
async fn apply_provider(ctx: Context, deps: ProviderDeps) -> Result<(), BoxError> {
    deps.computer_use.register(&ctx, mcp())?;
    Ok(())
}

#[test]
fn exports_the_source_identity_and_brands_without_validation() {
    assert_eq!(<ComputerUseRegistry as Plugin>::NAME, "computer-use");
    assert_eq!(<ComputerUse as ServiceKey>::NAME, "computerUse");
    assert_eq!(computer_use_provider_name("").as_str(), "");
    assert_eq!(
        computer_use_provider_name("provider-owned").as_str(),
        "provider-owned"
    );
}

#[tokio::test]
async fn plugin_publishes_the_registry_service_for_its_lifetime() {
    let kernel = Kernel::start();
    let root = kernel.root();
    let registry = root
        .plugin::<ComputerUseRegistry>(serde_json::json!({}))
        .unwrap();
    registry.settled().await.unwrap();
    assert!(root.get::<ComputerUse>().is_some());

    registry.dispose().await;
    assert!(root.get::<ComputerUse>().is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_a_second_provider_and_permits_registration_after_disposal() {
    let kernel = Kernel::start();
    let root = kernel.root();
    let registry_fiber = root
        .plugin::<ComputerUseRegistry>(serde_json::json!({}))
        .unwrap();
    registry_fiber.settled().await.unwrap();
    let registry = root.get::<ComputerUse>().unwrap();

    assert_eq!(registry.provider_name(), None);
    let dispose = registry.register(&root, mcp()).unwrap();
    assert_eq!(registry.provider_name(), Some(mcp()));

    let same = match registry.register(&root, mcp()) {
        Ok(_) => panic!("a repeated provider registration must fail"),
        Err(error) => error,
    };
    assert_eq!(
        same.to_string(),
        "computer use provider \"cua-driver-mcp\" is already registered"
    );
    let different = match registry.register(&root, native()) {
        Ok(_) => panic!("a different provider registration must fail"),
        Err(error) => error,
    };
    assert_eq!(
        different.to_string(),
        "computer use provider \"cua-driver-mcp\" is already registered"
    );

    dispose.dispose().await;
    assert_eq!(registry.provider_name(), None);
    let dispose_native = registry.register(&root, native()).unwrap();
    assert_eq!(registry.provider_name(), Some(native()));
    dispose_native.dispose().await;
    assert_eq!(registry.provider_name(), None);
    kernel.shutdown().await;
}

#[tokio::test]
async fn releases_a_provider_contribution_when_its_plugin_unloads() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<ComputerUseRegistry>(serde_json::json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let provider = root
        .plugin::<TestComputerUseProvider>(serde_json::json!({}))
        .unwrap();
    provider.settled().await.unwrap();
    let registry = root.get::<ComputerUse>().unwrap();
    assert_eq!(registry.provider_name(), Some(mcp()));

    provider.dispose().await;
    assert_eq!(registry.provider_name(), None);
    let native = registry.register(&root, native()).unwrap();
    assert_eq!(
        registry.provider_name(),
        Some(computer_use_provider_name("cua-driver-native"))
    );
    native.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn inactive_owner_cannot_occupy_the_provider_slot() {
    let kernel = Kernel::start();
    let registry = ComputerUseRegistry::default();
    let owner = kernel.root();
    kernel.shutdown().await;

    assert!(registry.register(&owner, mcp()).is_err());
    assert_eq!(registry.provider_name(), None);
}

#[tokio::test]
async fn duplicate_diagnostic_precedes_an_inactive_callers_error() {
    let kernel = Kernel::start();
    let registry = ComputerUseRegistry::default();
    let incumbent = registry.register(&kernel.root(), mcp()).unwrap();
    let inactive = kernel.root().child("inactive-provider").unwrap();
    inactive.fiber().dispose().await;

    let error = match registry.register(&inactive, native()) {
        Ok(_) => panic!("an inactive contender must not replace the incumbent"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "computer use provider \"cua-driver-mcp\" is already registered"
    );
    assert_eq!(registry.provider_name(), Some(mcp()));

    incumbent.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_registrations_publish_exactly_one_provider() {
    let kernel = Kernel::start();
    let registry = ComputerUseRegistry::default();
    let barrier = Arc::new(Barrier::new(3));
    let attempts = [mcp(), native()].map(|name| {
        let registry = registry.clone();
        let context = kernel.root();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            let result = registry.register(&context, name.clone());
            (name, result)
        })
    });
    barrier.wait();
    let [first, second] = attempts.map(|attempt| attempt.join().unwrap());
    let outcomes = [first, second];
    assert_eq!(
        outcomes.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    let incumbent = registry.provider_name().expect("published provider");
    let loser = outcomes
        .iter()
        .find_map(|(_, result)| result.as_ref().err())
        .expect("rejected contender");
    assert_eq!(
        loser.to_string(),
        format!("computer use provider \"{incumbent}\" is already registered")
    );
    let disposer = outcomes
        .into_iter()
        .find_map(|(_, result)| result.ok())
        .expect("winning disposer");
    disposer.dispose().await;
    assert_eq!(registry.provider_name(), None);
    kernel.shutdown().await;
}
