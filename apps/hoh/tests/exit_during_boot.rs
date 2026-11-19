//! `shutdown` can arrive while the plugin tree is still finishing its boot: the server raises the
//! exit flag (`mark_exiting`) and disposes the kernel root. The boot then fails on the disposed
//! root, and `run_profile` must treat that as the requested exit, not as a load failure.
mod support;
use hoh::profile_boot::*;
use hoh_kernel::{Cfg, Context, Error, KernelError, NoConfig, Plugin};
use hoh_launch_environment::create_launch_environment_snapshot;
use hoh_loader::{
    Layer, LayerSource, LoadError, PluginRegistry, parse_patches,
    startup_diagnostics::{ActivationErrors, StartupCompositionDiagnostic, StartupError},
};
use std::{path::Path, sync::Arc};

struct Guest(ProfileHost);
hoh_kernel::service!(GuestKey: Guest = "testGuest");

/// Does what the server does after a `shutdown` response: raises the flag, disposes the root.
struct ExitsMidBoot;
impl Plugin for ExitsMidBoot {
    const NAME: &'static str = "exits-mid-boot";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        ctx.get::<GuestKey>()
            .ok_or("missing host")?
            .0
            .mark_exiting();
        ctx.root().fiber().dispose().await;
        Ok(())
    }
}
/// A required row that is still mounting when the kernel closes (disposal waits for it to finish).
struct Hangs;
impl Plugin for Hangs {
    const NAME: &'static str = "hangs";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        Ok(())
    }
}
/// The same disposal with no exit request: a plain crash of the kernel.
struct DisposesWithoutExit;
impl Plugin for DisposesWithoutExit {
    const NAME: &'static str = "disposes-without-exit";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        ctx.root().fiber().dispose().await;
        Ok(())
    }
}
/// A genuine failure after the exit flag went up.
struct ExitsThenFails;
impl Plugin for ExitsThenFails {
    const NAME: &'static str = "exits-then-fails";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        ctx.get::<GuestKey>()
            .ok_or("missing host")?
            .0
            .mark_exiting();
        Err("a real failure".into())
    }
}

fn options(root: &Path, id: &str, name: &str, registry: PluginRegistry) -> RunProfileOptions {
    let mut profile = support::profile(root, "desktop", &[]);
    let file = profile.dir.join("bundle.yml");
    profile.layers.push(Layer {
        source: LayerSource::Bundle("fixture".into()),
        file: Some(file.clone()),
        patches: parse_patches(&format!("- insert: [{{id: {id}, name: {name}}}]\n"), &file)
            .unwrap(),
    });
    RunProfileOptions {
        profile,
        patch_files: vec![],
        home: root.join("home"),
        bundles: Arc::new(ShippedBundles),
        environment: create_launch_environment_snapshot(&[]),
        env: support::env(root),
        args: vec![],
        registry: Arc::new(registry),
        install_root: None,
    }
}

async fn boot_with<P: Plugin>(
    id: &str,
    name: &str,
    closer: impl FnOnce(ProfileHost) -> Option<tokio::task::JoinHandle<()>>,
) -> Result<RunningProfile, Box<dyn std::error::Error + Send + Sync>> {
    let root = tempfile::tempdir().unwrap();
    let mut registry = PluginRegistry::default();
    registry.add::<P>(name);
    let host = ProfileHost::new(Arc::new(|_| {}), Arc::new(|_| {}));
    host.kernel
        .root()
        .provide::<GuestKey>(Arc::new(Guest(host.clone())))
        .await
        .unwrap();
    let closer = closer(host.clone());
    let result = run_profile(options(root.path(), id, name, registry), host).await;
    if let Some(closer) = closer {
        closer.await.unwrap();
    }
    result
}

/// Closes the kernel root from outside while a row is still mounting, as the server does after a
/// `shutdown` response; `exiting` is whether the flag is raised first.
fn closes_the_root(
    exiting: bool,
) -> impl FnOnce(ProfileHost) -> Option<tokio::task::JoinHandle<()>> {
    move |host| {
        Some(tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if exiting {
                host.mark_exiting();
            }
            host.kernel.root().fiber().dispose().await;
        }))
    }
}

// The boot ends with an `InactiveContext` when the disposed row is the one that closes the root.
#[tokio::test(start_paused = true)]
async fn a_root_disposed_after_the_exit_flag_is_a_requested_exit() {
    let running = boot_with::<ExitsMidBoot>("exiting", "exits-mid-boot", |_| None)
        .await
        .expect("a requested exit is not a load failure");
    assert!(running.loader.is_none());
}

// The root is closed from outside, as the server does, while a required row is still mounting.
#[tokio::test(start_paused = true)]
async fn a_root_closed_from_outside_after_the_exit_flag_is_a_requested_exit() {
    let running = boot_with::<Hangs>("agent-loop", "hangs", closes_the_root(true))
        .await
        .expect("closed required rows after an exit request are not a load failure");
    assert!(running.loader.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_root_disposed_without_the_exit_flag_is_still_an_error() {
    let error = boot_with::<DisposesWithoutExit>("exiting", "disposes-without-exit", |_| None)
        .await
        .err()
        .expect("without an exit request the disposed root is a failure");
    assert!(error.to_string().contains("inactive context"), "{error}");
    let error = boot_with::<Hangs>("agent-loop", "hangs", closes_the_root(false))
        .await
        .err()
        .expect("without an exit request a closed root is a failure");
    assert!(error.to_string().contains("inactive context"), "{error}");
}

fn startup_failure(causes: Option<Vec<KernelError>>) -> LoadError {
    LoadError::Startup(Box::new(StartupError {
        message: "hoh: startup failed".into(),
        configuration_path: None,
        composition: StartupCompositionDiagnostic {
            plugins: vec![],
            services: vec![],
            omitted_plugins: 0,
            omitted_services: 0,
        },
        failures: vec![],
        messages: vec![],
        entries: vec![],
        cause: causes.map(|errors| ActivationErrors { errors }),
    }))
}

// When the kernel closes before the final audit, every required row fails with `Closed`, and the
// boot ends with a startup failure instead of `InactiveContext`. That is the requested exit too,
// but only with the flag up and only when nothing but `Closed` is behind it.
#[tokio::test]
async fn a_startup_failure_made_only_of_closed_rows_is_a_requested_exit_once_the_flag_is_up() {
    let host = ProfileHost::new(Arc::new(|_| {}), Arc::new(|_| {}));
    let closed = || startup_failure(Some(vec![KernelError::Closed, KernelError::Closed]));
    assert!(!stopped_by_exit_request(&host, &closed()));
    host.mark_exiting();
    assert!(stopped_by_exit_request(&host, &closed()));
    let nested = LoadError::Boot(Box::new(closed()));
    assert!(stopped_by_exit_request(&host, &nested));
    let failed = startup_failure(Some(vec![
        KernelError::Closed,
        KernelError::Failed(Arc::new("boom".into())),
    ]));
    assert!(!stopped_by_exit_request(&host, &failed));
    assert!(!stopped_by_exit_request(&host, &startup_failure(None)));
    assert!(!stopped_by_exit_request(
        &host,
        &startup_failure(Some(vec![]))
    ));
}

#[tokio::test(start_paused = true)]
async fn a_real_failure_after_the_exit_flag_is_still_an_error() {
    let error = boot_with::<ExitsThenFails>("webserver", "exits-then-fails", |_| None)
        .await
        .err()
        .expect("a plugin failure is not hidden by an exit request");
    assert!(error.to_string().contains("a real failure"), "{error}");
}
