//! Shared helpers of the credentials-local specs: temp homes, booting the provider under a kernel,
//! launch-environment layers and event/log capture.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

use hoh_credentials::{
    CredentialKey, CredentialProvider, CredentialRef, Credentials, CredentialsRecordUpdated,
    CredentialsReferenceUpdated, credential_ref,
};
use hoh_credentials_local::{Config, LocalCredentialProvider, Seams};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Fiber, Kernel, Level, Plugin};
use hoh_launch_environment::{
    LaunchEnvironment, LaunchEnvironmentLayerInput, LaunchEnvironmentSource,
    create_launch_environment_snapshot,
};
use serde_json::{Value, json};

/// A credentials document is seeded owner-only, exactly as the provider creates it.
pub fn write_credentials(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

pub fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

pub fn temp_dir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::Builder::new()
        .prefix("dsh-credentials-local-")
        .tempdir()
        .unwrap();
    // The provider refuses to store under a directory other users can enter, and a temporary
    // directory follows the umask.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

pub fn doc(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join(".credentials.yaml")
}

pub fn reference(name: &str) -> CredentialRef {
    credential_ref(name).unwrap()
}

/// Seams a test hands to its provider, looked up by the document path in the config.
static SEAMS: LazyLock<Mutex<HashMap<String, Seams>>> = LazyLock::new(Mutex::default);

pub fn register_seams(path: &Path, seams: Seams) {
    SEAMS
        .lock()
        .unwrap()
        .insert(path.to_string_lossy().into_owned(), seams);
}

/// The provider plugin with test seams: what the production plugin does, with the writer and the
/// watcher replaceable.
pub struct SeamedCredentials;

impl Plugin for SeamedCredentials {
    const NAME: &'static str = "SeamedCredentials";
    type Config = Config;
    type Deps = ();

    async fn apply(ctx: Context, _deps: (), config: Cfg<Config>) -> Result<(), BoxError> {
        let config = config.get();
        let seams = config
            .path
            .as_ref()
            .and_then(|path| SEAMS.lock().unwrap().get(path).cloned())
            .unwrap_or_default();
        let provider = LocalCredentialProvider::start(&ctx, &config, seams).await?;
        ctx.provide::<Credentials>(provider).await?;
        Ok(())
    }
}

pub struct Booted {
    pub kernel: Kernel,
    pub ctx: Context,
    pub fiber: Fiber,
}

impl Booted {
    pub fn credentials(&self) -> hoh_kernel::Service<Credentials> {
        self.ctx.get::<Credentials>().expect("credentials service")
    }

    pub async fn shutdown(self) {
        self.fiber.dispose().await;
        self.kernel.shutdown().await;
    }
}

pub fn layer(
    source: LaunchEnvironmentSource,
    path: Option<&str>,
    values: &[(&str, &str)],
) -> LaunchEnvironmentLayerInput {
    LaunchEnvironmentLayerInput {
        source,
        path: path.map(str::to_owned),
        values: values
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect(),
    }
}

/// Start a kernel with an optional launch environment, without mounting the provider yet.
pub async fn kernel_with(layers: Option<&[LaunchEnvironmentLayerInput]>) -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    if let Some(layers) = layers {
        ctx.provide::<LaunchEnvironment>(Arc::new(create_launch_environment_snapshot(layers)))
            .await
            .unwrap();
    }
    (kernel, ctx)
}

/// Mount the production plugin and wait for its activation.
pub async fn boot(config: Value) -> Booted {
    try_boot(None, config)
        .await
        .unwrap_or_else(|failure| panic!("boot failed: {failure}"))
}

pub async fn boot_layered(config: Value, layers: &[LaunchEnvironmentLayerInput]) -> Booted {
    try_boot(Some(layers), config)
        .await
        .unwrap_or_else(|failure| panic!("boot failed: {failure}"))
}

pub async fn try_boot(
    layers: Option<&[LaunchEnvironmentLayerInput]>,
    config: Value,
) -> Result<Booted, String> {
    let (kernel, ctx) = kernel_with(layers).await;
    let fiber = ctx
        .plugin::<LocalCredentialProvider>(config)
        .map_err(|error| error.to_string())?;
    match fiber.settled().await {
        Ok(()) => Ok(Booted { kernel, ctx, fiber }),
        Err(error) => {
            let message = error.to_string();
            kernel.shutdown().await;
            Err(message)
        }
    }
}

/// Mount the plugin with test seams registered for the config's path.
pub async fn boot_seamed(config: Value, seams: Seams) -> Booted {
    let path = config["path"].as_str().expect("config path").to_owned();
    register_seams(Path::new(&path), seams);
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx.plugin::<SeamedCredentials>(config).unwrap();
    fiber
        .settled()
        .await
        .unwrap_or_else(|failure| panic!("boot failed: {failure}"));
    Booted { kernel, ctx, fiber }
}

pub fn config(path: &Path) -> Value {
    json!({ "path": path.to_string_lossy(), "watch": false })
}

pub fn watching(path: &Path, debounce_ms: u64) -> Value {
    json!({ "path": path.to_string_lossy(), "debounceMs": debounce_ms })
}

/// Reference-updated events seen so far.
pub fn updates(ctx: &Context) -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    ctx.on_contained::<CredentialsReferenceUpdated>(move |event| {
        sink.lock().unwrap().push(event.reference.to_string());
        Ok(())
    })
    .unwrap();
    seen
}

pub fn record_updates(ctx: &Context) -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    ctx.on_contained::<CredentialsRecordUpdated>(move |event| {
        sink.lock().unwrap().push(event.key.to_string());
        Ok(())
    })
    .unwrap();
    seen
}

pub fn snapshot(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    seen.lock().unwrap().clone()
}

/// Warn-level journal messages so far.
pub fn warnings(ctx: &Context) -> Vec<String> {
    ctx.logger()
        .buffer()
        .into_iter()
        .filter(|record| record.level == Level::Warn)
        .map(|record| record.message)
        .collect()
}

/// Poll `check` until it holds, like `vi.waitFor`.
pub async fn wait_for<F, Fut>(mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if check().await {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not reached in 5s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

pub async fn pause(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

pub async fn resolved(
    credentials: &dyn CredentialProvider,
    name: &str,
) -> Option<(String, String)> {
    credentials
        .resolve(&reference(name))
        .await
        .unwrap()
        .map(|resolved| (resolved.value, resolved.source))
}

pub fn key(scope: &str, id: &str) -> CredentialKey {
    hoh_credentials::credential_key(scope, id).unwrap()
}
