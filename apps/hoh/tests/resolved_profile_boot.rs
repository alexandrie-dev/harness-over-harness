mod support;
use hoh::process_shutdown::PROCESS_SHUTDOWN_TIMEOUT_MS;
use hoh::profile_boot::*;
use hoh_kernel::{Cfg, Context, Error, NoConfig, Plugin, Undo};
use hoh_launch_environment::create_launch_environment_snapshot;
use hoh_loader::cmdline::{AppExitKey, AppReadyKey, CmdlineArgsKey};
use hoh_loader::{Layer, LayerSource, PluginRegistry, Profile, ProfileContextKey, parse_patches};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
#[derive(Default)]
struct Seen {
    values: Mutex<Vec<Value>>,
    disposed: AtomicUsize,
    proxy: Mutex<Option<Arc<ProxyTransport>>>,
    continue_mount: tokio::sync::Notify,
}
hoh_kernel::service!(SeenKey: Seen = "testSeen");
struct Target;
impl Plugin for Target {
    const NAME: &'static str = "target";
    type Config = Value;
    type Deps = ();
    async fn apply(ctx: Context, _: (), config: Cfg<Value>) -> Result<(), Error> {
        let seen = ctx.get::<SeenKey>().ok_or("missing test state")?;
        *seen.proxy.lock().unwrap() = ctx.tag::<ProxyTransport>();
        seen.values.lock().unwrap().push((*config.get()).clone());
        ctx.effect("fixture cleanup", move || {
            Undo::sync(move || {
                seen.disposed.fetch_add(1, Ordering::AcqRel);
            })
        })?;
        Ok(())
    }
}
struct Fails;
impl Plugin for Fails {
    const NAME: &'static str = "fails";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        if let Some(seen) = ctx.get::<SeenKey>() {
            *seen.proxy.lock().unwrap() = ctx.tag::<ProxyTransport>();
        }
        Err("startup failed".into())
    }
}
struct ExitThenFails;
impl Plugin for ExitThenFails {
    const NAME: &'static str = "exit-then-fails";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        ctx.get::<AppExitKey>().ok_or("missing appExit")?.request(7);
        Err("failure after exit request".into())
    }
}
struct RequestsExit;
impl Plugin for RequestsExit {
    const NAME: &'static str = "requests-exit";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        ctx.get::<AppExitKey>().ok_or("missing appExit")?.request(7);
        Ok(())
    }
}
struct RequestsExitThenHangs;
impl Plugin for RequestsExitThenHangs {
    const NAME: &'static str = "requests-exit-then-hangs";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        let seen = ctx.get::<SeenKey>().ok_or("missing test state")?;
        seen.continue_mount.notified().await;
        ctx.get::<AppExitKey>().ok_or("missing appExit")?.request(9);
        std::future::pending::<()>().await;
        Ok(())
    }
}

struct MountedBeforeExit;
impl Plugin for MountedBeforeExit {
    const NAME: &'static str = "mounted-before-exit";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        let seen = ctx.get::<SeenKey>().ok_or("missing test state")?;
        ctx.effect("mounted row cleanup", move || {
            Undo::sync(move || {
                seen.disposed.fetch_add(1, Ordering::AcqRel);
            })
        })?;
        Ok(())
    }
}
fn layer(file: &Path, source: LayerSource, text: &str) -> Layer {
    Layer {
        source,
        file: Some(file.into()),
        patches: parse_patches(text, file).unwrap(),
    }
}
fn options(root: &Path, profile: Profile, registry: PluginRegistry) -> RunProfileOptions {
    RunProfileOptions {
        profile,
        patch_files: vec![],
        home: root.join("home"),
        bundles: Arc::new(ShippedBundles),
        environment: create_launch_environment_snapshot(&[]),
        env: support::env(root),
        args: vec!["--port".into(), "0".into(), "--no-open".into()],
        registry: Arc::new(registry),
        install_root: None,
    }
}
fn host() -> ProfileHost {
    ProfileHost::new(Arc::new(|_| {}), Arc::new(|_| {}))
}
#[tokio::test(start_paused = true)]
async fn a_signal_before_profile_setup_returns_disposed_without_readiness_or_a_startup_error() {
    let root = tempfile::tempdir().unwrap();
    let profile = support::profile(root.path(), "desktop", &[]);
    let host = host();
    let ready = Arc::new(AtomicUsize::new(0));
    let observe = ready.clone();
    let _listener = host.ready.service().on_ready(Arc::new(move || {
        observe.fetch_add(1, Ordering::AcqRel);
    }));
    host.interrupt(0);
    host.shutdown.shutdown(0).await;
    let running = run_profile(
        options(root.path(), profile, PluginRegistry::default()),
        host,
    )
    .await
    .unwrap();
    assert!(running.ctx.fiber().is_disposed());
    assert!(running.loader.is_none());
    assert_eq!(ready.load(Ordering::Acquire), 0);
}
#[tokio::test(start_paused = true)]
async fn a_proxy_diagnostic_write_failure_keeps_its_error_and_releases_the_host() {
    let root = tempfile::tempdir().unwrap();
    let profile = support::profile(root.path(), "desktop", &[]);
    let mut options = options(root.path(), profile, PluginRegistry::default());
    options.environment = create_launch_environment_snapshot(&[
        hoh_launch_environment::LaunchEnvironmentLayerInput {
            source: hoh_launch_environment::LaunchEnvironmentSource::Process,
            path: None,
            values: [("HTTPS_PROXY".into(), "invalid-proxy".into())].into(),
        },
    ]);
    let error = Box::new(std::io::Error::other("proxy diagnostic stream closed"));
    let address = (&*error as *const std::io::Error) as usize;
    let error = Arc::new(Mutex::new(Some(error)));
    let host = host().with_reporter(Arc::new(move |text| {
        assert_eq!(
            text,
            "dsh: HTTPS_PROXY is not a valid URL; connecting directly\n"
        );
        Err(error.lock().unwrap().take().unwrap() as hoh_error::BoxError)
    }));
    let kernel = host.kernel.clone();
    let context = host.context().clone();
    let error = run_profile(options, host)
        .await
        .err()
        .unwrap()
        .downcast::<std::io::Error>()
        .unwrap();
    assert_eq!((&*error as *const std::io::Error) as usize, address);
    assert!(context.fiber().is_disposed());
    assert!(matches!(
        kernel.root().ensure_active(),
        Err(hoh_kernel::KernelError::Closed)
    ));
}
#[tokio::test(start_paused = true)]
async fn releases_startup_resources_after_a_preparation_failure() {
    let root = tempfile::tempdir().unwrap();
    let profile = support::profile(root.path(), "desktop", &[]);
    std::fs::create_dir(profile.dir.join("cordis.yml")).unwrap();
    let host = host();
    let kernel = host.kernel.clone();
    let context = host.context().clone();
    assert!(
        run_profile(
            options(root.path(), profile, PluginRegistry::default()),
            host
        )
        .await
        .is_err()
    );
    assert!(context.fiber().is_disposed());
    assert!(matches!(
        kernel.root().ensure_active(),
        Err(hoh_kernel::KernelError::Closed)
    ));
}
#[tokio::test(start_paused = true)]
async fn releases_startup_resources_after_a_boot_failure() {
    let root = tempfile::tempdir().unwrap();
    let mut profile = support::profile(root.path(), "desktop", &[]);
    let file = profile.dir.join("bundle.yml");
    profile.layers.push(layer(
        &file,
        LayerSource::Bundle("fixture".into()),
        "- insert: [{id: webserver, name: fails}]\n",
    ));
    let mut registry = PluginRegistry::default();
    registry.add::<Fails>("fails");
    let host = host();
    let kernel = host.kernel.clone();
    let context = host.context().clone();
    let seen = Arc::new(Seen::default());
    kernel
        .root()
        .provide::<SeenKey>(seen.clone())
        .await
        .unwrap();
    let result = run_profile(options(root.path(), profile, registry), host).await;
    let error = result.err().expect("boot must fail");
    assert!(error.to_string().contains("startup failed"), "{error}");
    assert!(context.fiber().is_disposed());
    assert!(matches!(
        kernel.root().ensure_active(),
        Err(hoh_kernel::KernelError::Closed)
    ));
    assert!(
        seen.proxy
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .installation()
            .is_none()
    );
}
#[tokio::test(start_paused = true)]
async fn an_app_exit_requested_during_mount_does_not_hide_the_row_failure() {
    let root = tempfile::tempdir().unwrap();
    let mut profile = support::profile(root.path(), "desktop", &[]);
    let file = profile.dir.join("bundle.yml");
    profile.layers.push(layer(
        &file,
        LayerSource::Bundle("fixture".into()),
        "- insert: [{id: webserver, name: exit-then-fails}]\n",
    ));
    let mut registry = PluginRegistry::default();
    registry.add::<ExitThenFails>("exit-then-fails");
    let error = run_profile(options(root.path(), profile, registry), host())
        .await
        .err()
        .expect("row failure must escape startup");
    assert!(
        error.to_string().contains("failure after exit request"),
        "{error}"
    );
}
#[tokio::test(start_paused = true)]
async fn a_successful_mount_honors_the_exit_requested_during_startup() {
    let root = tempfile::tempdir().unwrap();
    let mut profile = support::profile(root.path(), "desktop", &[]);
    let file = profile.dir.join("bundle.yml");
    profile.layers.push(layer(
        &file,
        LayerSource::Bundle("fixture".into()),
        "- insert: [{id: webserver, name: requests-exit}]\n",
    ));
    let mut registry = PluginRegistry::default();
    registry.add::<RequestsExit>("requests-exit");
    let completed = Arc::new(Mutex::new(vec![]));
    let observed = completed.clone();
    let host = ProfileHost::new(
        Arc::new(|code| panic!("unexpected forced exit {code}")),
        Arc::new(move |code| observed.lock().unwrap().push(code)),
    );
    let running = run_profile(options(root.path(), profile, registry), host)
        .await
        .unwrap();
    for _ in 0..16 {
        if !completed.lock().unwrap().is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(*completed.lock().unwrap(), [7]);
    assert!(running.ctx.fiber().is_disposed());
}
#[tokio::test(start_paused = true)]
async fn an_exit_request_disposes_mounted_rows_while_a_later_mount_is_still_pending() {
    let root = tempfile::tempdir().unwrap();
    let mut profile = support::profile(root.path(), "desktop", &[]);
    let file = profile.dir.join("bundle.yml");
    profile.layers.push(layer(
        &file,
        LayerSource::Bundle("fixture".into()),
        "- insert:\n    - {id: mounted, name: mounted-before-exit}\n    - {id: webserver, name: requests-exit-then-hangs}\n",
    ));
    let mut registry = PluginRegistry::default();
    registry
        .add::<MountedBeforeExit>("mounted-before-exit")
        .add::<RequestsExitThenHangs>("requests-exit-then-hangs");
    let forced = Arc::new(Mutex::new(vec![]));
    let observed_force = forced.clone();
    let completed = Arc::new(Mutex::new(vec![]));
    let observed_complete = completed.clone();
    let host = ProfileHost::new(
        Arc::new(move |code| observed_force.lock().unwrap().push(code)),
        Arc::new(move |code| observed_complete.lock().unwrap().push(code)),
    );
    let kernel = host.kernel.clone();
    let seen = Arc::new(Seen::default());
    host.kernel
        .root()
        .provide::<SeenKey>(seen.clone())
        .await
        .unwrap();
    let launch = run_profile(options(root.path(), profile, registry), host);
    tokio::pin!(launch);
    for _ in 0..256 {
        tokio::select! {
            result = &mut launch => panic!("mount settled before appExit: {}", result.is_ok()),
            () = tokio::task::yield_now() => {}
        }
        if kernel.snapshot().await.fibers.iter().any(|fiber| {
            fiber.name.as_ref() == "mounted-before-exit"
                && fiber.state == hoh_kernel::FiberState::Active
        }) {
            break;
        }
    }
    assert!(kernel.snapshot().await.fibers.iter().any(|fiber| {
        fiber.name.as_ref() == "mounted-before-exit"
            && fiber.state == hoh_kernel::FiberState::Active
    }));
    seen.continue_mount.notify_one();
    for _ in 0..256 {
        tokio::select! {
            result = &mut launch => panic!("hanging mount unexpectedly settled: {}", result.is_ok()),
            () = tokio::task::yield_now() => {}
        }
        if seen.disposed.load(Ordering::Acquire) == 1 {
            break;
        }
    }
    assert_eq!(seen.disposed.load(Ordering::Acquire), 1);
    assert!(forced.lock().unwrap().is_empty());
    assert!(completed.lock().unwrap().is_empty());
    tokio::select! {
        result = &mut launch => panic!("hanging mount unexpectedly settled: {}", result.is_ok()),
        () = tokio::time::sleep(Duration::from_millis(PROCESS_SHUTDOWN_TIMEOUT_MS)) => {}
    }
    for _ in 0..8 {
        if !forced.lock().unwrap().is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(*forced.lock().unwrap(), [9]);
}
#[tokio::test(start_paused = true)]
async fn uses_shared_layers_and_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let mut profile = support::profile(root.path(), "desktop", &[]);
    let seen = Arc::new(Seen::default());
    std::fs::write(profile.dir.join("cordis.yml"), "- id: stale\n").unwrap();
    for (source, name, text) in [
        (
            LayerSource::Bundle("fixture".into()),
            "bundle.yml",
            "- insert: [{id: target, name: target, config: {priority: bundle}}]\n",
        ),
        (
            LayerSource::Profile,
            "profile.yml",
            "- id: target\n  config: {priority: profile}\n",
        ),
    ] {
        profile
            .layers
            .push(layer(&profile.dir.join(name), source, text));
    }
    std::fs::create_dir_all(root.path().join("home")).unwrap();
    std::fs::write(
        root.path().join("home/cordis.patch.yml"),
        "- id: target\n  config: {priority: home}\n",
    )
    .unwrap();
    let overlay = root.path().join("overlay.yml");
    std::fs::write(&overlay, "- id: target\n  config: {priority: overlay}\n").unwrap();
    let dir = profile.dir.clone();
    let mut registry = PluginRegistry::default();
    registry.add::<Target>("target");
    let mut opts = options(root.path(), profile, registry);
    opts.patch_files.push(overlay);
    opts.environment = create_launch_environment_snapshot(&[
        hoh_launch_environment::LaunchEnvironmentLayerInput {
            source: hoh_launch_environment::LaunchEnvironmentSource::Process,
            path: None,
            values: [("HTTPS_PROXY".into(), "http://localhost:8080".into())].into(),
        },
    ]);
    let host = host();
    host.kernel
        .root()
        .provide::<SeenKey>(seen.clone())
        .await
        .unwrap();
    let running = run_profile(opts, host).await.unwrap();
    let transport = running.ctx.tag::<ProxyTransport>().unwrap();
    assert_eq!(
        transport
            .installation()
            .unwrap()
            .policy()
            .https_proxy
            .as_deref(),
        Some("http://localhost:8080")
    );
    let builder = running
        .ctx
        .tag::<hoh_web_fetch_http::HttpFetchClientBuilder>()
        .unwrap();
    assert!((builder.0)().is_ok());
    assert_eq!(
        std::fs::read_to_string(dir.join("cordis.yml")).unwrap(),
        PROFILE_ROOT_CONFIG
    );
    let row = running
        .loader
        .as_ref()
        .unwrap()
        .entries()
        .into_iter()
        .find(|r| r.options.id == "target")
        .unwrap();
    assert_eq!(
        *row.fiber.unwrap().effective_config().unwrap(),
        json!({"priority":"overlay"})
    );
    assert_eq!(
        running.ctx.get::<CmdlineArgsKey>().unwrap().get(),
        ["--port", "0", "--no-open"]
    );
    let ready = Arc::new(AtomicUsize::new(0));
    let observe = ready.clone();
    let _subscription = running
        .ctx
        .get::<AppReadyKey>()
        .unwrap()
        .on_ready(Arc::new(move || {
            observe.fetch_add(1, Ordering::AcqRel);
        }));
    assert_eq!(ready.load(Ordering::Acquire), 1);
    let profile = running.ctx.get::<ProfileContextKey>().unwrap();
    assert_eq!(profile.name, "desktop");
    assert_eq!(profile.dir, dir);
    assert_eq!(profile.home, root.path().join("home"));
    assert_eq!(
        *seen.values.lock().unwrap(),
        [json!({"priority":"overlay"})]
    );
    running.host.shutdown.shutdown(0).await;
    assert!(running.ctx.fiber().is_disposed());
    assert_eq!(seen.disposed.load(Ordering::Acquire), 1);
    assert!(transport.installation().is_none());
    assert!((builder.0)().is_err());
}
#[test]
fn encodes_profile_names() {
    for (name, expected) in [
        ("web", "web"),
        ("sdk-minimal", "sdk-minimal"),
        ("my web/v2", "my%20web%2Fv2"),
        ("工作", "%E5%B7%A5%E4%BD%9C"),
        ("a(b)%", "a%28b%29%25"),
    ] {
        assert_eq!(user_agent_profile(name), expected);
    }
}
#[tokio::test(start_paused = true)]
async fn names_the_profile_before_any_plugin_loads() {
    struct Observe;
    impl Plugin for Observe {
        const NAME: &'static str = "observe";
        type Config = Value;
        type Deps = ();
        async fn apply(_: Context, _: (), config: Cfg<Value>) -> Result<(), Error> {
            assert_eq!(config.get()["suffix"], "dsh-profile/my%20web");
            Ok(())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut profile = support::profile(root.path(), "my web", &[]);
    let file = profile.dir.join("bundle.yml");
    profile.layers.push(layer(&file,LayerSource::Bundle("fixture".into()),"- insert:\n    - id: observe\n      name: observe\n      config:\n        suffix: !!js process.env.DSH_USER_AGENT_SUFFIX\n"));
    let mut registry = PluginRegistry::default();
    registry.add::<Observe>("observe");
    let running = run_profile(options(root.path(), profile, registry), host())
        .await
        .unwrap();
    running.host.shutdown.shutdown(0).await;
}
