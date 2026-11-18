//! Port of @deepseek-ai/dsh-storage-domain (packages/storage/storage-domain).
//! Schema-validated domains over the storage hub's routed KV backends.
pub mod domain;
pub mod error;
pub mod events;
pub mod spec;
pub use domain::{Domain, DomainFuture, DomainGlobal, DomainImpl, KvTable};
pub use error::{DomainError, DomainErrorCode, DomainErrorOptions, InvalidRecordDetail, ZodError};
pub use events::DomainChanged;
pub use spec::{
    DomainGlobalHandleOf, DomainGlobalSpec, DomainSpec, DomainSpecInput, DomainTableSpec,
    ErasedGlobalSpec, ErasedTableSpec, GlobalValueOf, InvalidRecords, TableKeyOf, TableValueOf,
    ValueSchema, define_domain, descriptor_of, domain_table,
};

use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Cfg, Context, Inject, PluginDef, PluginOptions, Service, Undo, plugin, service};
use hoh_storage::{KvUnit, Storage, StorageBackend, storage_backend_service_key};
use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, MutexGuard},
};

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Config {
    /// Default backend name for every domain without an explicit route. Required: there is no universally correct medium.
    pub backend: String,
    /// Per-domain overrides: domain name → backend name.
    #[serde(default)]
    pub routes: IndexMap<String, String>,
}
#[derive(Default)]
struct FacilityState {
    domains: IndexMap<String, Arc<Domain>>,
    reserved: IndexSet<String>,
}
struct FacilityInner {
    ctx: Context,
    config: Config,
    state: Mutex<FacilityState>,
}
#[derive(Clone)]
pub struct DomainFacility {
    inner: Arc<FacilityInner>,
}
impl DomainFacility {
    pub fn new(ctx: Context, config: Config) -> Self {
        Self {
            inner: Arc::new(FacilityInner {
                ctx,
                config,
                state: Mutex::new(FacilityState::default()),
            }),
        }
    }
    pub fn open(&self, spec: Arc<DomainSpec>) -> DomainFuture<Arc<Domain>> {
        {
            let mut state = lock(&self.inner.state);
            if !state.reserved.insert(spec.name.clone()) {
                return domain::ready(Err(SharedError::new(DomainError::new(
                    DomainErrorCode::AlreadyOpen,
                    format!("domain '{}' is already open", spec.name),
                ))));
            }
        }
        let name = self
            .inner
            .config
            .routes
            .get(&spec.name)
            .unwrap_or(&self.inner.config.backend)
            .clone();
        let backend = self
            .inner
            .ctx
            .get::<Storage>()
            .ok_or_else(|| error::plain("service \"storage\" is not registered"))
            .and_then(|storage| storage.backend.get(&name).map_err(SharedError::new));
        let backend = match backend {
            Err(error) => {
                lock(&self.inner.state).reserved.shift_remove(&spec.name);
                return domain::ready(Err(error));
            }
            Ok(backend) if backend.kv().is_none() => {
                lock(&self.inner.state).reserved.shift_remove(&spec.name);
                return domain::ready(Err(SharedError::new(DomainError::new(
                    DomainErrorCode::FacetUnsupported,
                    format!(
                        "backend '{name}' routed for domain '{}' has no kv facet",
                        spec.name
                    ),
                ))));
            }
            Ok(backend) => backend,
        };
        let inner = self.inner.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        // As with a JS Promise, dropping the awaiting future does not cancel open.
        tokio::spawn(async move {
            let result = inner
                .open_domain(&spec, backend.as_ref())
                .await
                .map_err(SharedError::new);
            if result.is_err() {
                lock(&inner.state).reserved.shift_remove(&spec.name);
            }
            let _ = tx.send(result);
        });
        Box::pin(async move {
            rx.await
                .unwrap_or_else(|_| Err(error::plain("domain open task stopped")))
        })
    }
    pub fn get(&self, name: &str) -> Option<Arc<Domain>> {
        lock(&self.inner.state).domains.get(name).cloned()
    }
    pub fn close_all(&self) -> DomainFuture<()> {
        let futures = lock(&self.inner.state)
            .domains
            .values()
            .map(|domain| domain.close())
            .collect::<Vec<_>>();
        Box::pin(async move {
            futures_util::future::try_join_all(futures)
                .await
                .map(|_| ())
        })
    }
}

impl FacilityInner {
    async fn open_domain(
        self: &Arc<Self>,
        spec: &DomainSpec,
        backend: &dyn StorageBackend,
    ) -> Result<Arc<Domain>, BoxError> {
        // The kv facet was checked immediately before starting this open.
        let unit = backend
            .kv()
            .expect("backend facets are immutable")
            .open(descriptor_of(spec))
            .await?;
        match self.load_domain(spec, unit.clone()).await {
            Ok(domain) => Ok(domain),
            Err(error) => {
                unit.close().await?;
                Err(error)
            }
        }
    }

    async fn load_domain(
        self: &Arc<Self>,
        spec: &DomainSpec,
        unit: Arc<dyn KvUnit>,
    ) -> Result<Arc<Domain>, BoxError> {
        let snapshot = unit.load_all().await?;
        let mut tables = IndexMap::new();
        for (table, table_spec) in &spec.tables {
            let mut records = IndexMap::new();
            if let Some(saved) = snapshot.tables.get(table) {
                for key in js_keys(saved) {
                    match parse_record(&spec.name, table, &key, || {
                        (table_spec.parser)(&saved[&key])
                    }) {
                        Ok(value) => {
                            records.insert(key, value);
                        }
                        Err(error) => {
                            if spec.invalid_records != Some(InvalidRecords::BackupAndSkip) {
                                return Err(Box::new(error));
                            }
                            let Some(backup) = unit.backup_record(table, &key) else {
                                return Err(Box::new(error));
                            };
                            let moved = backup.await?;
                            let cause = error.cause().map_or_else(
                                || "undefined".into(),
                                |cause| error::js_error_string(cause),
                            );
                            self.ctx.logger().error(format!(
                                "domain '{}': stored record '{key}' in table '{table}' failed schema validation; moved to '{moved}' and treated as absent. Cause: {cause}",
                                spec.name
                            ));
                        }
                    }
                }
            }
            tables.insert(table.clone(), records);
        }
        let global = match &spec.global {
            None => None,
            Some(global) if snapshot.global.is_null() => Some(global.initial.clone()),
            Some(global) => Some(parse_record(&spec.name, "", "", || {
                (global.parser)(&snapshot.global)
            })?),
        };
        let owner = Arc::downgrade(self);
        let name = spec.name.clone();
        let domain = Domain::new(
            self.ctx.clone(),
            spec,
            unit.clone(),
            tables,
            global,
            move || {
                if let Some(owner) = owner.upgrade() {
                    let mut state = lock(&owner.state);
                    state.domains.shift_remove(&name);
                    state.reserved.shift_remove(&name);
                }
            },
        );
        lock(&self.state)
            .domains
            .insert(spec.name.clone(), domain.clone());
        Ok(domain)
    }
}

fn parse_record<T>(
    domain: &str,
    table: &str,
    key: &str,
    parse: impl FnOnce() -> Result<T, BoxError>,
) -> Result<T, DomainError> {
    let parsed = catch_unwind(AssertUnwindSafe(parse))
        .unwrap_or_else(|payload| Err(error::panic_error(payload)));
    parsed.map_err(|error| {
        let slot = if table.is_empty() {
            "global".into()
        } else {
            format!("record '{key}' in table '{table}'")
        };
        DomainError::with_options(
            DomainErrorCode::InvalidRecord,
            format!("domain '{domain}': stored {slot} does not match its schema"),
            DomainErrorOptions {
                detail: Some(InvalidRecordDetail {
                    table: table.into(),
                    key: key.into(),
                }),
                cause: Some(SharedError::new(error)),
            },
        )
    })
}
fn js_keys<T>(values: &IndexMap<String, T>) -> Vec<String> {
    let keys = Value::Object(
        values
            .keys()
            .map(|key| (key.clone(), Value::Null))
            .collect::<Map<_, _>>(),
    );
    // Shared stringify/parse implements Object.keys integer-index ordering.
    let ordered = hoh_util_values::js_json_parse(&hoh_util_values::js_json_stringify(&keys))
        .expect("serialized shallow JSON keys parse successfully");
    ordered
        .as_object()
        .into_iter()
        .flat_map(|map| map.keys().cloned())
        .collect()
}
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
service!(pub StorageDomain: DomainFacility = "storageDomain");
#[derive(Inject)]
pub struct Deps {
    storage: Service<Storage>,
}
#[plugin(StorageDomainPlugin, name = "storage-domain")]
pub async fn apply(ctx: Context, _deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
    let config = config.get();
    let mut names = IndexSet::new();
    names.insert(config.backend.clone());
    for key in js_keys(&config.routes) {
        names.insert(config.routes[&key].clone());
    }
    ctx.plugin_dyn(
        &PluginDef::of::<MountDomain>(),
        serde_json::to_value(config.as_ref())?,
        PluginOptions {
            inject: names
                .iter()
                .map(|name| storage_backend_service_key(name).into())
                .collect(),
            ..PluginOptions::default()
        },
    )?;
    Ok(())
}
#[plugin(MountDomain, name = "storage-domain")]
async fn mount(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
    let facility = Arc::new(DomainFacility::new(ctx.clone(), (*config.get()).clone()));
    let owner = facility.clone();
    ctx.try_effect("storage domain form", |undos| {
        let unmount = deps.storage.mount("domain", facility.clone())?;
        undos.push(Undo::future(move || async move {
            owner.close_all().await?;
            unmount();
            Ok(())
        }))?;
        Ok::<(), BoxError>(())
    })?;
    ctx.provide::<StorageDomain>(facility).await?;
    Ok(())
}
