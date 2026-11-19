//! Active Loader-backed plugin package inventory for official DeepSeek requests.

use hoh_agent::Agents;
use hoh_agent_preset_registry::AgentPresets;
use hoh_deepseek_llm_api_extensions::{
    DeepSeekLlmApiExtensionRequest, DeepSeekLlmApiExtensions, PreparedDeepSeekLlmApiExtension,
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, FiberState, Inject, Plugin, Service, ServiceKey};
use hoh_loader::{EntryInfo, LoaderKey, ProfileContextKey};
use hoh_session::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};
use url::Url;

/// Cordis plugin name.
pub const NAME: &str = "plugin-package-inventory-deepseek";
/// Services required by the source plugin.
pub const INJECT: &[&str] = &["agents", "deepseekLlmApiExtensions", "loader"];

const FIELD: &str = "dsh_plugin_packages";

type PackageLookup = dyn Fn(&str, &str) -> Option<PathBuf> + Send + Sync;
type EntryBaseUrl = dyn Fn(&EntryInfo) -> Option<String> + Send + Sync;

fn enabled() -> bool {
    true
}

/// Plugin-package request contribution configuration.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Config {
    /// Contribute `dsh_plugin_packages`; defaults to true.
    #[serde(default = "enabled")]
    pub enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// One active plugin package, including a version only when the manifest supplies one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeepSeekPluginPackageIdentity {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Versioned full inventory carried by each official DeepSeek request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeepSeekPluginPackageInventoryExtension {
    pub version: u8,
    pub packages: Vec<DeepSeekPluginPackageIdentity>,
}

/// Snapshot of app-boot's optional profile package service.
///
/// This fixed interface exists because `hoh-loader` does not currently expose the
/// source `PluginPackages` service. A provider is authoritative: a miss does not
/// fall back to a native `node_modules` walk.
#[derive(Clone)]
// SHIM(hoh-loader): replace with app-boot's owned package lookup once it is public.
pub struct PluginPackages {
    lookup: Arc<PackageLookup>,
}

impl PluginPackages {
    pub fn new(lookup: impl Fn(&str, &str) -> Option<PathBuf> + Send + Sync + 'static) -> Self {
        Self {
            lookup: Arc::new(lookup),
        }
    }

    fn package_of(&self, package: &str, anchor: &str) -> Option<PathBuf> {
        (self.lookup)(package, anchor)
    }
}

/// Optional app-boot package lookup under the source service name.
pub struct PluginPackagesKey;

impl ServiceKey for PluginPackagesKey {
    type Value = PluginPackages;
    const NAME: &'static str = "pluginPackages";
}

/// Exact Loader base facts supplied by a future Loader adapter.
///
/// `hoh-loader::EntryInfo` does not yet expose the declaring tree base. Without
/// this service the plugin uses the host base for ordinary entries.
#[derive(Clone)]
// SHIM(hoh-loader): EntryInfo does not expose its declaring tree base URL.
pub struct InventoryRuntime {
    pub host_base_url: String,
    entry_base_url: Arc<EntryBaseUrl>,
}

impl InventoryRuntime {
    pub fn new(
        host_base_url: impl Into<String>,
        entry_base_url: impl Fn(&EntryInfo) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            host_base_url: host_base_url.into(),
            entry_base_url: Arc::new(entry_base_url),
        }
    }
}

/// Optional Loader bridge used until `EntryInfo` carries its declaring base.
pub struct InventoryRuntimeKey;

impl ServiceKey for InventoryRuntimeKey {
    type Value = InventoryRuntime;
    const NAME: &'static str = "pluginPackageInventoryRuntime";
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveEntry {
    module_name: String,
    base_url: Option<String>,
    bare_base_url: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum InventoryError {
    #[error("plugin-package-inventory-deepseek: cannot resolve active package {0}")]
    CannotResolve(String),
    #[error("plugin-package-inventory-deepseek: invalid base URL {0:?}")]
    InvalidBase(String),
    #[error("{path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{path}: package manifest is null")]
    NullManifest { path: PathBuf },
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn bare_package_name(specifier: &str) -> Option<String> {
    if specifier.starts_with('.') || specifier.contains(':') || Path::new(specifier).is_absolute() {
        return None;
    }
    let mut parts = specifier.split('/');
    let first = parts.next().unwrap_or_default();
    if first.starts_with('@') {
        Some(format!("{first}/{}", parts.next().unwrap_or_default()))
    } else {
        Some(first.to_owned())
    }
}

fn identity_from_manifest(
    path: &Path,
) -> Result<Option<DeepSeekPluginPackageIdentity>, InventoryError> {
    let bytes = fs::read(path).map_err(|source| InventoryError::Read {
        path: path.to_owned(),
        source,
    })?;
    // Node's readFileSync(path, "utf8") replaces malformed byte sequences.
    let text = String::from_utf8_lossy(&bytes);
    let manifest: serde_json::Value =
        serde_json::from_str(&text).map_err(|source| InventoryError::Parse {
            path: path.to_owned(),
            source,
        })?;
    // `JSON.parse("null").name` throws in JavaScript; other primitive values
    // are boxed and simply expose no `name` property.
    if manifest.is_null() {
        return Err(InventoryError::NullManifest {
            path: path.to_owned(),
        });
    }
    let Some(object) = manifest.as_object() else {
        return Ok(None);
    };
    let Some(name) = object.get("name").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    if hoh_util_values::js_trim(name).is_empty() {
        return Ok(None);
    }
    let version = object
        .get("version")
        .and_then(serde_json::Value::as_str)
        .filter(|version| !hoh_util_values::js_trim(version).is_empty())
        .map(str::to_owned);
    Ok(Some(DeepSeekPluginPackageIdentity {
        name: name.to_owned(),
        version,
    }))
}

fn anchor_path(anchor: &str) -> Option<(PathBuf, bool)> {
    match Url::parse(anchor) {
        Ok(url) if url.scheme() == "file" => {
            let directory = url.path().ends_with('/');
            url.to_file_path().ok().map(|path| (path, directory))
        }
        Ok(_) => None,
        Err(_) => {
            let path = PathBuf::from(anchor);
            let directory = anchor.ends_with('/') || anchor.ends_with('\\');
            path.is_absolute().then_some((path, directory))
        }
    }
}

fn bare_package_manifest(
    package: &str,
    anchors: &[String],
    packages: Option<&PluginPackages>,
) -> Option<PathBuf> {
    for anchor in anchors {
        if let Some(packages) = packages {
            if let Some(manifest) = packages.package_of(package, anchor) {
                return Some(manifest);
            }
            continue;
        }
        let Some((anchor, directory_anchor)) = anchor_path(anchor) else {
            continue;
        };
        let mut current = if directory_anchor {
            Some(anchor.as_path())
        } else {
            anchor.parent()
        };
        while let Some(directory) = current {
            let manifest = directory
                .join("node_modules")
                .join(package)
                .join("package.json");
            if manifest.exists() {
                return Some(manifest);
            }
            current = directory.parent();
        }
    }
    None
}

fn normalize_absolute_path(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn nearest_manifest(module_path: &Path) -> Option<PathBuf> {
    let mut current = module_path.parent()?;
    loop {
        let manifest = current.join("package.json");
        if manifest.exists() {
            return Some(manifest);
        }
        current = current.parent()?;
    }
}

fn module_file(module_name: &str, tree_base: &str) -> Result<Option<PathBuf>, InventoryError> {
    if Path::new(module_name).is_absolute() {
        return Ok(Some(normalize_absolute_path(Path::new(module_name))));
    }
    let base = Url::parse(tree_base)
        .or_else(|_| Url::from_file_path(tree_base))
        .map_err(|_| InventoryError::InvalidBase(tree_base.to_owned()))?;
    let module = base
        .join(module_name)
        .map_err(|_| InventoryError::InvalidBase(tree_base.to_owned()))?;
    if module.scheme() != "file" {
        return Ok(None);
    }
    Ok(module.to_file_path().ok())
}

struct PackageIdentityResolver {
    host_base_url: String,
    module_base_url: String,
    packages: Option<PluginPackages>,
    cache: Mutex<HashMap<String, Option<DeepSeekPluginPackageIdentity>>>,
}

impl PackageIdentityResolver {
    fn new(host_base_url: String, packages: Option<PluginPackages>) -> Self {
        Self::with_module_base(host_base_url, package_module_url(), packages)
    }

    fn with_module_base(
        host_base_url: String,
        module_base_url: String,
        packages: Option<PluginPackages>,
    ) -> Self {
        Self {
            host_base_url,
            module_base_url,
            packages,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn resolve(
        &self,
        entry: &ActiveEntry,
    ) -> Result<Option<DeepSeekPluginPackageIdentity>, InventoryError> {
        let tree_base = entry.base_url.as_deref().unwrap_or(&self.host_base_url);
        let mut anchors = Vec::new();
        for anchor in [
            entry.bare_base_url.as_deref().unwrap_or(tree_base),
            tree_base,
            &self.host_base_url,
            &self.module_base_url,
        ] {
            if !anchors.iter().any(|present| present == anchor) {
                anchors.push(anchor.to_owned());
            }
        }
        let key = format!("{}\0{}", anchors.join("\0"), entry.module_name);
        if let Some(identity) = lock(&self.cache).get(&key).cloned() {
            return Ok(identity);
        }

        let manifest = if let Some(package) = bare_package_name(&entry.module_name) {
            Some(
                bare_package_manifest(&package, &anchors, self.packages.as_ref()).ok_or_else(
                    || {
                        InventoryError::CannotResolve(
                            serde_json::to_string(&package).expect("string serialization"),
                        )
                    },
                )?,
            )
        } else if entry.module_name.starts_with("cordis:") {
            None
        } else {
            module_file(&entry.module_name, tree_base)?.and_then(|path| nearest_manifest(&path))
        };
        let identity = manifest
            .as_deref()
            .map(identity_from_manifest)
            .transpose()?
            .flatten();
        lock(&self.cache).insert(key, identity.clone());
        Ok(identity)
    }
}

fn package_module_url() -> String {
    Url::from_file_path(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .map(Into::into)
        .unwrap_or_else(|()| "file:///hoh-plugin-package-inventory-deepseek/src/lib.rs".to_owned())
}

fn compare_wire_text(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

fn sort_and_deduplicate(
    identities: impl IntoIterator<Item = DeepSeekPluginPackageIdentity>,
) -> Vec<DeepSeekPluginPackageIdentity> {
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for identity in identities {
        let key = (identity.name.clone(), identity.version.clone());
        if seen.insert(key) {
            unique.push(identity);
        }
    }
    unique.sort_by(|left, right| {
        compare_wire_text(&left.name, &right.name).then_with(|| {
            compare_wire_text(
                left.version.as_deref().unwrap_or_default(),
                right.version.as_deref().unwrap_or_default(),
            )
        })
    });
    unique
}

fn fallback_host_base(ctx: &Context) -> String {
    ctx.get::<ProfileContextKey>()
        .and_then(|profile| Url::from_file_path(profile.dir.join("cordis.yml")).ok())
        .map(Into::into)
        .unwrap_or_else(package_module_url)
}

#[derive(Inject)]
pub struct Deps {
    agents: Service<Agents>,
    extensions: Service<DeepSeekLlmApiExtensions>,
    loader: Service<LoaderKey>,
}

struct Inventory {
    ctx: Context,
    agents: Service<Agents>,
    loader: Service<LoaderKey>,
    runtime: Option<InventoryRuntime>,
    host_base_url: String,
    resolver: PackageIdentityResolver,
    unreadable: Mutex<HashSet<String>>,
}

impl Inventory {
    fn host_entries(&self) -> Vec<ActiveEntry> {
        self.loader
            .entries()
            .into_iter()
            .filter(|entry| {
                !entry.options.group
                    && !entry.disabled
                    && entry
                        .fiber
                        .as_ref()
                        .is_some_and(|fiber| fiber.state() == FiberState::Active)
            })
            .map(|entry| ActiveEntry {
                base_url: self
                    .runtime
                    .as_ref()
                    .and_then(|runtime| (runtime.entry_base_url)(&entry))
                    .or_else(|| Some(self.host_base_url.clone())),
                module_name: entry.options.name,
                bare_base_url: None,
            })
            .collect()
    }

    async fn collect(&self, session_id: Option<&str>) -> Vec<DeepSeekPluginPackageIdentity> {
        let mut entries = self.host_entries();
        // Optional services are live context facts in the source; do not retain
        // a withdrawn registry or remember an initial miss forever.
        let presets = self.ctx.get::<AgentPresets>();
        if let (Some(session_id), Some(presets)) = (session_id, presets.as_ref())
            && let Some(agent) = self.agents.get(&SessionId::from(session_id))
        {
            for composition in presets.inspect_compositions(Some(agent.ctx())).await {
                entries.extend(composition.modules.into_iter().map(|module| ActiveEntry {
                    module_name: module.module_name,
                    base_url: module.base_url,
                    bare_base_url: module.use_host_base.then(|| self.host_base_url.clone()),
                }));
            }
        }

        let mut identities = Vec::new();
        for entry in entries {
            match self.resolver.resolve(&entry) {
                Ok(Some(identity)) => identities.push(identity),
                Ok(None) => {}
                Err(error) => {
                    let first = lock(&self.unreadable).insert(entry.module_name.clone());
                    if first {
                        self.ctx.logger().warn(format_args!(
                            "plugin-package-inventory-deepseek: omitting unreadable package identity for {}: {error}",
                            entry.module_name
                        ));
                    }
                }
            }
        }
        sort_and_deduplicate(identities)
    }
}

/// The Loader/agent inventory contributor.
pub struct PluginPackageInventoryDeepSeek;

impl Plugin for PluginPackageInventoryDeepSeek {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        if !config.get().enabled {
            return Ok(());
        }
        let runtime = ctx
            .get::<InventoryRuntimeKey>()
            .map(|runtime| (*runtime).clone());
        let host_base_url = runtime
            .as_ref()
            .map(|runtime| runtime.host_base_url.clone())
            .unwrap_or_else(|| fallback_host_base(&ctx));
        let packages = ctx
            .get::<PluginPackagesKey>()
            .map(|packages| (*packages).clone());
        let inventory = Arc::new(Inventory {
            ctx: ctx.clone(),
            agents: deps.agents.clone(),
            loader: deps.loader.clone(),
            runtime,
            resolver: PackageIdentityResolver::new(host_base_url.clone(), packages),
            host_base_url,
            unreadable: Mutex::new(HashSet::new()),
        });
        deps.extensions
            .register::<DeepSeekPluginPackageInventoryExtension, _>(
                &ctx,
                FIELD,
                move |request: Arc<DeepSeekLlmApiExtensionRequest>| {
                    let inventory = inventory.clone();
                    async move {
                        Ok(Some(PreparedDeepSeekLlmApiExtension {
                            value: DeepSeekPluginPackageInventoryExtension {
                                version: 1,
                                packages: inventory.collect(request.session_id.as_deref()).await,
                            },
                            accept: None,
                        }))
                    }
                },
            )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
