//! Shared boot of named or embedding-owned profiles, over the same empty root as dumps.
use super::{
    process_shutdown::{Exit, ProcessShutdown, create_process_shutdown},
    profile_initialization::{fs_error, initialize_profile_from_default},
    window_glue::{WINDOW_EXTERNAL_PROFILE, WINDOW_PROFILE},
};
use hoh_error::BoxError;
use hoh_kernel::{Context, FiberState, Kernel, Service, Undo};
use hoh_launch_environment::{LaunchEnvironment, LaunchEnvironmentSnapshot};
use hoh_loader::cmdline::{
    AppExit, AppReady, CmdlineHost, Listener, Subscription, provide_cmdline,
};
use hoh_loader::{
    Bundle, BundleSource, EvalEnv, LayerSource, LoadError, LoaderKey, PluginRegistry, Profile,
    ProfileContext, ProfileContextKey,
};
use indexmap::IndexMap;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
pub const PROFILE_ROOT_FILENAME: &str = "cordis.yml";
pub const PROFILE_ROOT_CONFIG: &str = "# dsh profile root — an empty entry list. The tree is composed as patches:\n# each bundle in package.json's dsh.profile.bundles, then cordis.patch.yml, then any\n# --patch overlays. Edit cordis.patch.yml, not this file.\n[]\n";
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
const WINDOW_BUNDLE: &str = "@hoh/window";
const WINDOW_EXTERNAL_BUNDLE: &str = "@hoh/window-external";

fn window_bundle(profile: &str) -> Option<&'static str> {
    match profile {
        WINDOW_PROFILE => Some(WINDOW_BUNDLE),
        WINDOW_EXTERNAL_PROFILE => Some(WINDOW_EXTERNAL_BUNDLE),
        _ => None,
    }
}

pub struct ShippedBundles;
impl BundleSource for ShippedBundles {
    fn bundle(&self, name: &str) -> Result<Bundle, LoadError> {
        let text = match name {
            "@deepseek-ai/dsh-base" => include_str!("../assets/bundle/base/cordis.patch.yml"),
            "@deepseek-ai/dsh-headless" => {
                include_str!("../assets/bundle/headless/cordis.patch.yml")
            }
            "@deepseek-ai/dsh-acp-app" => include_str!("../assets/bundle/acp-app/cordis.patch.yml"),
            "@deepseek-ai/dsh-web-app" => include_str!("../assets/bundle/web-app/cordis.patch.yml"),
            "@deepseek-ai/dsh-sdk-app" => include_str!("../assets/bundle/sdk-app/cordis.patch.yml"),
            "@deepseek-ai/dsh-sdk-minimal" => {
                include_str!("../assets/bundle/sdk-minimal/cordis.patch.yml")
            }
            WINDOW_BUNDLE | WINDOW_EXTERNAL_BUNDLE => {
                include_str!("../assets/bundle/window/cordis.patch.yml")
            }
            _ => {
                return Err(LoadError::Runtime(format!(
                    "bundle {} is not included in this HoH build",
                    hoh_util_values::js_json_stringify(&name.into())
                )));
            }
        };
        let mut patch = text.to_owned();
        if name == WINDOW_BUNDLE {
            patch.push_str(include_str!("../assets/bundle/window/native.patch.yml"));
        } else if name == WINDOW_EXTERNAL_BUNDLE {
            patch.push_str(&format!(
                "\n- id: sdk-app-startup\n  config:\n    profile: {WINDOW_EXTERNAL_PROFILE}\n"
            ));
        }
        if matches!(name, WINDOW_BUNDLE | WINDOW_EXTERNAL_BUNDLE) {
            patch.push_str(
                "\n- insert:\n    - id: plugin-inventory\n      name: 'hoh:plugin-inventory'\n",
            );
            for (id, _) in hoh_authored_plugins::PLUGINS {
                patch.push_str(&format!(
                    "    - id: author-{id}\n      name: 'hoh:plugin-{id}'\n"
                ));
            }
        }
        let mut files = vec![("cordis.patch.yml".into(), patch)];
        if name == "@deepseek-ai/dsh-web-app" {
            for (file, text) in [
                (
                    "presets/standard.patch.yml",
                    include_str!("../assets/bundle/web-app/presets/standard.patch.yml"),
                ),
                (
                    "presets/ptc.patch.yml",
                    include_str!("../assets/bundle/web-app/presets/ptc.patch.yml"),
                ),
                (
                    "presets/minimal.patch.yml",
                    include_str!("../assets/bundle/web-app/presets/minimal.patch.yml"),
                ),
                (
                    "presets/cordis.patch.yml",
                    include_str!("../assets/bundle/web-app/presets/cordis.patch.yml"),
                ),
            ] {
                files.push((file.into(), text.into()));
            }
        }
        Ok(Bundle {
            name: name.into(),
            dir: None,
            platforms: None,
            files,
        })
    }
}
pub fn home_patch_path(home: &Path) -> PathBuf {
    home.join("cordis.patch.yml")
}
pub fn prepare_profile(
    name: &str,
    user_layer: bool,
    from_default_profile: Option<&str>,
    home: &Path,
    bundles: &dyn BundleSource,
    env: &EvalEnv,
    warn: &mut dyn FnMut(String),
) -> Result<Profile, BoxError> {
    if let Some(template) = from_default_profile {
        initialize_profile_from_default(name, template, home, warn)?;
    }
    if let Some(bundle) = window_bundle(name) {
        let dir = hoh_loader::resolve_profile_dir(name, home)?;
        if !dir.join("package.json").exists() {
            hoh_loader::init_profile(&dir, &[bundle.into()])?;
        }
    }
    let mut profile =
        hoh_loader::load_profile_with_user_layer("dsh", name, bundles, home, &[], env, false)?;
    if user_layer {
        let path = profile.dir.join("cordis.patch.yml");
        if let Some(patches) = hoh_loader::load_named_optional_patches("dsh", &path)? {
            profile.layers.push(hoh_loader::Layer {
                source: LayerSource::Profile,
                file: Some(path),
                patches,
            });
        }
    }
    for skipped in &profile.skipped {
        warn(format!(
            "dsh: skipping profile bundle {}: {}\n",
            hoh_util_values::js_json_stringify(&skipped.name.clone().into()),
            skipped.reason
        ));
    }
    let root = profile.dir.join(PROFILE_ROOT_FILENAME);
    std::fs::write(&root, PROFILE_ROOT_CONFIG).map_err(|e| fs_error("open", &root, None, e))?;
    Ok(profile)
}
pub fn user_agent_profile(name: &str) -> String {
    let mut encoded = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
#[derive(Clone)]
pub struct Readiness(Arc<Mutex<ReadyState>>);
struct ReadyState {
    ready: bool,
    next: u64,
    listeners: IndexMap<u64, Listener>,
}
struct StartupGate {
    mounting: AtomicBool,
    changed: tokio::sync::Notify,
}
impl StartupGate {
    fn new() -> Self {
        Self {
            mounting: AtomicBool::new(true),
            changed: tokio::sync::Notify::new(),
        }
    }
    fn finish(&self) {
        if self.mounting.swap(false, Ordering::AcqRel) {
            self.changed.notify_waiters();
        }
    }
    async fn wait(&self) {
        loop {
            let changed = self.changed.notified();
            if !self.mounting.load(Ordering::Acquire) {
                return;
            }
            changed.await;
        }
    }
}
impl Default for Readiness {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(ReadyState {
            ready: false,
            next: 0,
            listeners: IndexMap::new(),
        })))
    }
}
impl Readiness {
    pub fn service(&self) -> AppReady {
        let state = self.0.clone();
        AppReady::new(move |listener| {
            let id = {
                let mut state = lock(&state);
                if state.ready {
                    drop(state);
                    listener();
                    return Subscription::new(|| {});
                }
                let id = state.next;
                state.next += 1;
                state.listeners.insert(id, listener);
                id
            };
            let state = state.clone();
            Subscription::new(move || {
                lock(&state).listeners.shift_remove(&id);
            })
        })
    }
    pub fn commit(&self) {
        let listeners = {
            let mut state = lock(&self.0);
            if state.ready {
                return;
            }
            state.ready = true;
            std::mem::take(&mut state.listeners)
        };
        for listener in listeners.values() {
            listener();
        }
    }
}
pub use crate::window_glue::WindowStdio;
pub type Reporter = Arc<dyn Fn(&str) -> Result<(), BoxError> + Send + Sync>;
#[derive(Clone)]
pub struct ProfileHost {
    pub kernel: Kernel,
    pub shutdown: ProcessShutdown,
    pub ready: Readiness,
    interrupted: Arc<AtomicBool>,
    exit_requested: Arc<AtomicBool>,
    startup: Arc<StartupGate>,
    report: Reporter,
    context: Context,
    window_stdio: Arc<Mutex<Option<WindowStdio>>>,
}
impl ProfileHost {
    pub fn new(force_exit: Exit, complete: Exit) -> Self {
        let kernel = Kernel::start();
        // A fresh kernel has an active root and an unused first child uid.
        let context = kernel
            .root()
            .child("hoh-profile")
            .expect("fresh profile context");
        let startup = Arc::new(StartupGate::new());
        let wait_for_startup = startup.clone();
        let dispose = context.clone();
        let shutdown = create_process_shutdown(
            Arc::new(move || {
                let context = dispose.clone();
                let startup = wait_for_startup.clone();
                Box::pin(async move {
                    let active = context
                        .get::<LoaderKey>()
                        .map(|loader| {
                            loader
                                .entries()
                                .into_iter()
                                .filter(|entry| {
                                    !entry.options.group && entry.name != "cordis:include"
                                })
                                .filter_map(|entry| entry.fiber)
                                .filter(|fiber| fiber.state() == FiberState::Active)
                                .map(|fiber| async move { fiber.dispose().await })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    futures_util::future::join_all(active).await;
                    startup.wait().await;
                    context.fiber().dispose().await;
                    Ok(())
                })
            }),
            force_exit,
            complete,
            None,
        );
        let logger = kernel.root().logger();
        Self {
            kernel,
            shutdown,
            ready: Readiness::default(),
            interrupted: Arc::new(AtomicBool::new(false)),
            exit_requested: Arc::new(AtomicBool::new(false)),
            startup,
            report: Arc::new(move |text| {
                logger.warn(text);
                Ok(())
            }),
            context,
            window_stdio: Arc::default(),
        }
    }
    /// The disposable profile scope; uid 0 remains Active by kernel rule R15.
    pub fn context(&self) -> &Context {
        &self.context
    }
    /// Streams for the window profile instead of the process stdio (for an embedding host or a test).
    #[must_use]
    pub fn with_window_stdio(self, stdio: WindowStdio) -> Self {
        *self
            .window_stdio
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stdio);
        self
    }
    /// Whether the exit has been requested (`mark_exiting` or `request_exit`).
    pub fn exit_requested(&self) -> bool {
        self.exit_requested.load(Ordering::Acquire)
    }
    pub fn with_reporter(mut self, report: Reporter) -> Self {
        self.report = report;
        self
    }
    pub fn interrupt(&self, code: i32) {
        self.interrupted.store(true, Ordering::Release);
        self.startup.finish();
        self.shutdown.interrupt(code);
    }
    /// Records that the exit is requested, without starting the shutdown. The window engine calls
    /// it just before it disposes the kernel root, so a boot still finishing sees why the root went
    /// away; `request_exit` follows once the disposal is done.
    pub fn mark_exiting(&self) {
        self.exit_requested.store(true, Ordering::Release);
    }
    pub fn request_exit(&self, code: i32) {
        self.exit_requested.store(true, Ordering::Release);
        drop(self.shutdown.shutdown(code));
    }
    fn finish_startup(&self) {
        self.startup.finish();
    }
}
pub struct RunProfileOptions {
    pub profile: Profile,
    pub patch_files: Vec<PathBuf>,
    pub home: PathBuf,
    pub bundles: Arc<dyn BundleSource>,
    pub environment: LaunchEnvironmentSnapshot,
    pub env: EvalEnv,
    pub args: Vec<String>,
    pub registry: Arc<PluginRegistry>,
    /// Root of the installation; `DSH_INSTALL_DIR` is derived from it. `None` provides no
    /// `installAnchor`, and shell-env falls back to the executable's directory.
    pub install_root: Option<PathBuf>,
}
/// The installation root of a launched executable: the prefix above `bin/`, else its own directory.
pub fn install_root_of(exe: &Path) -> PathBuf {
    let dir = exe.parent().unwrap_or(exe);
    match (dir.file_name(), dir.parent()) {
        (Some(name), Some(prefix)) if name == "bin" => prefix.to_path_buf(),
        _ => dir.to_path_buf(),
    }
}
#[derive(Clone)]
pub(crate) struct EditorHost {
    pub dir: PathBuf,
    pub home: PathBuf,
    pub bundles: Arc<dyn BundleSource>,
    pub env: EvalEnv,
    pub overlays: Vec<hoh_loader::Layer>,
}
pub struct RunningProfile {
    pub ctx: Context,
    pub loader: Option<Service<LoaderKey>>,
    pub host: ProfileHost,
}
/// Owned native replacement for Node's process-global outbound dispatcher.
#[derive(Clone)]
pub struct ProxyTransport(Arc<Mutex<Option<Arc<hoh_http_proxy::ProxyInstallation>>>>);
impl ProxyTransport {
    pub fn installation(&self) -> Option<Arc<hoh_http_proxy::ProxyInstallation>> {
        lock(&self.0).clone()
    }
}
fn install_transport(
    ctx: &Context,
    environment: &LaunchEnvironmentSnapshot,
    report: &dyn Fn(&str) -> Result<(), BoxError>,
) -> Result<Context, BoxError> {
    use hoh_launch_environment::LaunchEnvironmentSource;
    let inherited = hoh_http_proxy::policy::PROXY_ENV_NAMES
        .into_iter()
        .map(|name| {
            (
                name.into(),
                environment
                    .get_from(name, &[LaunchEnvironmentSource::Process])
                    .map(|value| value.value),
            )
        })
        .collect();
    let lookup = |name: &str| environment.get(name).map(|value| value.value);
    let mut report_error = None;
    let installation = hoh_http_proxy::ProxyInstallation::with_inherited_environment(
        &lookup,
        inherited,
        |message| {
            if report_error.is_none()
                && let Err(error) = report(&format!("dsh: {message}\n"))
            {
                report_error = Some(error);
            }
        },
        Default::default(),
    )?;
    if let Some(error) = report_error {
        return Err(error);
    }
    let transport = ProxyTransport(Arc::new(Mutex::new(Some(Arc::new(installation)))));
    let dispose = transport.clone();
    ctx.effect("profile outbound transport", move || {
        Undo::sync(move || {
            lock(&dispose.0).take();
        })
    })?;
    let factory = transport.clone();
    let builder = hoh_web_fetch_http::HttpFetchClientBuilder(Arc::new(move || {
        let installation = factory
            .installation()
            .ok_or("profile outbound transport is disposed")?;
        hoh_http_proxy::route_client_builder(installation.policy().clone(), installation.options())
            .map_err(|error| Box::new(error) as BoxError)
    }));
    Ok(ctx.with_tag(transport).with_tag(builder))
}
pub async fn run_profile(
    mut options: RunProfileOptions,
    host: ProfileHost,
) -> Result<RunningProfile, BoxError> {
    if host.interrupted.load(Ordering::Acquire) {
        host.finish_startup();
        host.kernel.shutdown().await;
        return Ok(RunningProfile {
            ctx: host.context.clone(),
            loader: None,
            host,
        });
    }
    let suffix = format!("dsh-profile/{}", user_agent_profile(&options.profile.name));
    let previous = options.env.var.clone();
    options.env.var = Arc::new(move |name| {
        if name == "DSH_USER_AGENT_SUFFIX" {
            Some(suffix.clone())
        } else {
            previous(name)
        }
    });
    let root = match install_transport(&host.context, &options.environment, host.report.as_ref()) {
        Ok(context) => context,
        Err(error) => {
            host.finish_startup();
            host.kernel.shutdown().await;
            if host.interrupted.load(Ordering::Acquire) {
                return Ok(RunningProfile {
                    ctx: host.context.clone(),
                    loader: None,
                    host,
                });
            }
            return Err(error);
        }
    };
    options.profile.layers = match hoh_loader::read_named_profile_patches(
        "dsh",
        &options.profile,
        &options.home,
        &options.patch_files,
        &options.env,
    ) {
        Ok(layers) => layers,
        Err(error) => {
            host.finish_startup();
            host.kernel.shutdown().await;
            if host.interrupted.load(Ordering::Acquire) {
                return Ok(RunningProfile {
                    ctx: root,
                    loader: None,
                    host,
                });
            }
            return Err(Box::new(error));
        }
    };
    let path = options.profile.dir.join(PROFILE_ROOT_FILENAME);
    if let Err(error) = tokio::fs::write(&path, PROFILE_ROOT_CONFIG).await {
        host.finish_startup();
        host.kernel.shutdown().await;
        if host.interrupted.load(Ordering::Acquire) {
            return Ok(RunningProfile {
                ctx: root,
                loader: None,
                host,
            });
        }
        return Err(fs_error("open", &path, None, error));
    }
    let overlays = options
        .profile
        .layers
        .iter()
        .filter_map(|l| match &l.source {
            LayerSource::CommandLine(_) => Some(l.clone()),
            _ => None,
        })
        .collect();
    let ctx = root.with_tag(EditorHost {
        dir: options.profile.dir.clone(),
        home: options.home.clone(),
        bundles: options.bundles,
        env: options.env.clone(),
        overlays,
    });
    // The sdk profiles speak the same stdio JSON-RPC as the window engine, so they take the same glue
    // (single stdin reader, end-of-input announcement, exit after `shutdown`).
    let stdio_profile = window_bundle(&options.profile.name).is_some()
        || matches!(options.profile.name.as_str(), "sdk" | "sdk-minimal");
    let launch = async {
        ctx.provide::<ProfileContextKey>(Arc::new(ProfileContext {
            name: options.profile.name,
            dir: options.profile.dir,
            home: options.home,
        }))
        .await?;
        // `dirname(profile.installAnchor)` of the source is the installation root; the anchor is
        // the `package.json` that marks it.
        if let Some(root) = options.install_root {
            ctx.provide::<hoh_shell_env::InstallAnchorKey>(Arc::new(hoh_shell_env::InstallAnchor(
                root.join("package.json"),
            )))
            .await?;
        }
        ctx.provide::<LaunchEnvironment>(Arc::new(options.environment))
            .await?;
        let exit_host = host.clone();
        provide_cmdline(
            &ctx,
            CmdlineHost {
                args: options.args,
                exit: AppExit::new(move |code| exit_host.request_exit(code)),
                ready: Some(host.ready.service()),
            },
        )
        .await?;
        if stdio_profile {
            let (exiting_host, exit_host) = (host.clone(), host.clone());
            let stdio = host
                .window_stdio
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            crate::window_glue::provide_stdio(
                &ctx,
                stdio,
                move || exiting_host.mark_exiting(),
                move |code| exit_host.request_exit(code),
            )
            .await
            .map_err(|error| LoadError::Runtime(error.to_string()))?;
        }
        // dsh's profile boot keeps the profile root `cordis.yml`; the replay basename swap
        // (`resolveConfigPath`) belongs to the explicit config-path entry, not to a profile.
        let base = path.clone();
        hoh_loader::boot(
            &ctx,
            hoh_loader::BootOptions {
                base,
                layers: options.profile.layers,
                registry: options.registry,
                env: options.env,
            },
        )
        .await
    }
    .await;
    match launch {
        Ok(loader) => {
            host.finish_startup();
            if !host.exit_requested.load(Ordering::Acquire)
                && !host.interrupted.load(Ordering::Acquire)
                && ctx.fiber().state() == FiberState::Active
            {
                host.ready.commit();
            }
            Ok(RunningProfile {
                ctx,
                loader: Some(loader),
                host,
            })
        }
        Err(error) => {
            host.finish_startup();
            host.kernel.shutdown().await;
            if host.interrupted.load(Ordering::Acquire) {
                Ok(RunningProfile {
                    ctx,
                    loader: None,
                    host,
                })
            } else if stopped_by_exit_request(&host, &error) {
                // The error is dropped as a consequence of the requested exit; keep its text.
                host.kernel.root().logger().warn(format!(
                    "plugin tree load ended by the requested exit: {error}"
                ));
                Ok(RunningProfile {
                    ctx,
                    loader: None,
                    host,
                })
            } else {
                Err(Box::new(error))
            }
        }
    }
}
/// The window engine may be told to `shutdown` while the plugin tree is still finishing its boot.
/// After the response the server raises the exit flag (`mark_exiting`) and only then disposes the
/// kernel root, so the boot ends either with `InactiveContext` or, when the final audit finds every
/// row closed, with a startup failure whose causes are all `Closed`. Both are the requested exit.
/// Without the raised flag, and for any failure with another cause, the error stays an error.
pub fn stopped_by_exit_request(host: &ProfileHost, error: &LoadError) -> bool {
    if !host.exit_requested.load(Ordering::Acquire) {
        return false;
    }
    let mut error = error;
    while let LoadError::Boot(inner) = error {
        error = inner;
    }
    match error {
        LoadError::Kernel(hoh_kernel::KernelError::Inactive) => true,
        LoadError::Startup(startup) => startup.cause.as_ref().is_some_and(|cause| {
            !cause.errors.is_empty()
                && cause
                    .errors
                    .iter()
                    .all(|error| matches!(error, hoh_kernel::KernelError::Closed))
        }),
        _ => false,
    }
}
