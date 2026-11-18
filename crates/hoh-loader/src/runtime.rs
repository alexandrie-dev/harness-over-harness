use crate::{
    Composition, Entry, EvalEnv, Expr, Isolate, Layer, LoadError, PluginRegistry, compose,
    expression, is_truthy, parse_entries, parse_patches, render_dump,
};
use futures_util::future::join_all;
use hoh_kernel::{
    Cfg, ConfigResolver, Context, Error, Fiber, FiberInfo, FiberState, KernelError, Plugin,
    PluginOptions, Realm, Service, Undo,
};
use indexmap::{IndexMap, IndexSet};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
fn component(id: &str) -> String {
    id.replace('%', "%25").replace(':', "%3A")
}
fn addressed_key(id: &str) -> String {
    id.split(':').map(component).collect::<Vec<_>>().join(":")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryStatus {
    Disabled,
    Unresolved,
    Mounted,
}
#[derive(Clone)]
pub struct EntryInfo {
    pub id: String,
    pub name: String,
    pub group: Option<String>,
    pub disabled: bool,
    pub options: Entry,
    pub fiber: Option<Fiber>,
    pub status: EntryStatus,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryChange {
    Created(String),
    Updated(String, UpdateAction),
    Removed(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    None,
    Volatile,
    Config,
    Rebuilt,
    Enabled,
    Disabled,
}
pub struct Audit {
    pub missing: Vec<String>,
    pub unresolved: Vec<String>,
    pub inactive: Vec<FiberInfo>,
}
pub struct BootOptions {
    pub registry: Arc<PluginRegistry>,
    pub base: PathBuf,
    pub layers: Vec<Layer>,
    pub env: EvalEnv,
}
hoh_kernel::service!(pub LoaderKey: Loader = "loader");
pub struct ConfigReload;
hoh_kernel::event!(ConfigReload = "app-boot/config-reload", hoh_kernel::Emit);

#[derive(Clone)]
pub(crate) struct Node {
    pub(crate) info: EntryInfo,
    ctx: Context,
    generation: u64,
    error: Option<String>,
    update_error: Option<String>,
    pub(crate) source: PathBuf,
    dependencies: Vec<String>,
    cause: Option<KernelError>,
    tree_prefix: String,
    owner: Weak<GroupInner>,
}
#[derive(Default)]
struct TreeState {
    nodes: IndexMap<String, Node>,
    fibres: IndexMap<u32, String>,
    groups: IndexMap<String, Weak<GroupInner>>,
    labels: IndexMap<(String, String), Realm>,
    sequence: u64,
    changes: Vec<EntryChange>,
    hooked: bool,
    disposing: IndexSet<u32>,
    identities: IndexMap<(String, PathBuf), crate::identity::Identities>,
    bases: IndexMap<String, Vec<Entry>>,
    refreshes: IndexSet<String>,
}
#[derive(Clone)]
struct Site {
    tree: Arc<Mutex<TreeState>>,
    owner: Option<String>,
    prefix: String,
    owner_key: Option<String>,
    key_prefix: String,
    base: PathBuf,
    includes: Vec<PathBuf>,
}
struct GroupInner {
    ctx: Context,
    registry: Arc<PluginRegistry>,
    env: EvalEnv,
    site: Site,
    rows: Mutex<Vec<Entry>>,
    anonymous: Mutex<crate::identity::Identities>,
    version: Mutex<u64>,
    options: Mutex<IndexMap<String, Entry>>,
    closed: AtomicBool,
}
#[derive(Clone)]
pub struct Group {
    inner: Arc<GroupInner>,
}

impl Expr {
    pub fn eval(
        &self,
        env: &EvalEnv,
        ctx: &Context,
        registry: &PluginRegistry,
        base_dir: &Path,
    ) -> Result<Option<Value>, crate::ExprError> {
        crate::expr::evaluate(
            self,
            env,
            base_dir,
            &|name| {
                ctx.has(name)
                    .then(|| ctx.view(name).unwrap_or_else(|| json!({})))
            },
            &|name| registry.resource(name),
        )
    }
}

struct Resolver {
    env: EvalEnv,
    registry: Arc<PluginRegistry>,
    base: PathBuf,
}
impl ConfigResolver for Resolver {
    fn resolve(&self, ctx: &Context, raw: &Value) -> Result<Value, Error> {
        self.value(ctx, raw).map(|v| v.unwrap_or(Value::Null))
    }
}
impl Resolver {
    fn value(&self, ctx: &Context, raw: &Value) -> Result<Option<Value>, Error> {
        if let Some(source) = expression(raw) {
            return Ok(Expr::parse(source)?.eval(&self.env, ctx, &self.registry, &self.base)?);
        }
        Ok(Some(match raw {
            Value::Array(array) => Value::Array(
                array
                    .iter()
                    .map(|v| self.value(ctx, v).map(|v| v.unwrap_or(Value::Null)))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Object(object) => {
                let mut result = serde_json::Map::new();
                for (key, value) in object {
                    if let Some(value) = self.value(ctx, value)? {
                        result.insert(key.clone(), value);
                    }
                }
                Value::Object(result)
            }
            value => value.clone(),
        }))
    }
}

impl Group {
    pub fn mount(
        ctx: &Context,
        registry: Arc<PluginRegistry>,
        env: EvalEnv,
    ) -> Result<Group, KernelError> {
        let site = ctx
            .tag::<Site>()
            .map(|v| (*v).clone())
            .unwrap_or_else(|| Site {
                tree: Arc::new(Mutex::new(TreeState::default())),
                owner: None,
                prefix: String::new(),
                owner_key: None,
                key_prefix: String::new(),
                base: env.cwd.clone(),
                includes: vec![],
            });
        Self::at(ctx, registry, env, site)
    }
    fn at(
        ctx: &Context,
        registry: Arc<PluginRegistry>,
        env: EvalEnv,
        site: Site,
    ) -> Result<Self, KernelError> {
        ctx.ensure_active()?;
        let ctx = ctx.with_tag(Runtime {
            registry: registry.clone(),
            env: env.clone(),
        });
        let group = Self {
            inner: Arc::new(GroupInner {
                ctx: ctx.clone(),
                registry,
                env,
                site,
                rows: Mutex::new(vec![]),
                anonymous: Mutex::new(crate::identity::Identities::default()),
                version: Mutex::new(0),
                options: Mutex::new(IndexMap::new()),
                closed: AtomicBool::new(false),
            }),
        };
        let key = group.key();
        lock(&group.inner.site.tree)
            .groups
            .insert(key.clone(), Arc::downgrade(&group.inner));
        let hook = {
            let mut tree = lock(&group.inner.site.tree);
            let hook = !tree.hooked;
            tree.hooked = true;
            hook
        };
        if hook {
            let tree = Arc::downgrade(&group.inner.site.tree);
            ctx.on::<hoh_kernel::InternalStatus>(move |event| {
                if event.fiber.state() != FiberState::Disposed {
                    return;
                }
                let Some(tree) = tree.upgrade() else {
                    return;
                };
                let mut tree = lock(&tree);
                if tree.disposing.contains(&event.fiber.uid()) {
                    return;
                }
                if let Some(key) = tree.fibres.get(&event.fiber.uid()).cloned()
                    && let Some(node) = tree.nodes.get_mut(&key)
                    && node
                        .info
                        .fiber
                        .as_ref()
                        .is_some_and(|f| f.uid() == event.fiber.uid())
                    && !node.info.disabled
                    && node.ctx.fiber().state() == FiberState::Active
                {
                    node.info.disabled = true;
                    node.info.options.disabled = Value::Bool(true);
                    node.info.status = EntryStatus::Disabled;
                }
            })?;
        }
        let owned = group.clone();
        ctx.effect("loader group", move || {
            Undo::future(move || async move {
                owned.stop().await;
                Ok(())
            })
        })?;
        Ok(group)
    }
    fn key(&self) -> String {
        self.inner.site.owner_key.clone().unwrap_or_default()
    }
    fn full_id(&self, id: &str) -> String {
        format!("{}{id}", self.inner.site.prefix)
    }
    fn row_key(&self, id: &str) -> String {
        format!("{}{}", self.inner.site.key_prefix, component(id))
    }
    pub fn entries(&self) -> Vec<EntryInfo> {
        self.records().into_iter().map(|node| node.info).collect()
    }
    fn records(&self) -> Vec<Node> {
        let mut out = vec![];
        let mut seen = IndexSet::new();
        let mut pending: Vec<_> = lock(&self.inner.rows)
            .iter()
            .rev()
            .map(|row| (self.row_key(&row.id), Arc::downgrade(&self.inner)))
            .collect();
        while let Some((id, parent)) = pending.pop() {
            let (node, child) = {
                let tree = lock(&self.inner.site.tree);
                (
                    tree.nodes
                        .get(&id)
                        .filter(|n| Weak::ptr_eq(&n.owner, &parent))
                        .cloned(),
                    tree.groups.get(&id).and_then(Weak::upgrade),
                )
            };
            if let Some(node) = node
                && seen.insert(id)
            {
                out.push(node);
                if let Some(inner) = child {
                    let group = Group { inner };
                    pending.extend(
                        lock(&group.inner.rows)
                            .iter()
                            .rev()
                            .map(|row| (group.row_key(&row.id), Arc::downgrade(&group.inner))),
                    );
                }
            }
        }
        out
    }
    pub async fn update(&self, rows: Vec<Entry>) -> Vec<EntryChange> {
        self.run(self.prepare(rows)).await
    }
    async fn update_guarded(
        &self,
        rows: Vec<Entry>,
        generation: &Mutex<u64>,
        expected: u64,
    ) -> Vec<EntryChange> {
        let plan = {
            let current = lock(generation);
            if *current != expected {
                return vec![];
            }
            self.prepare(rows)
        };
        self.run(plan).await
    }
    fn prepare(&self, mut rows: Vec<Entry>) -> (u64, Vec<(String, Option<Entry>)>) {
        let mut version = lock(&self.inner.version);
        *version += 1;
        if self.inner.closed.load(Ordering::Acquire) || self.inner.ctx.ensure_active().is_err() {
            return (*version, vec![]);
        }
        let existing = lock(&self.inner.site.tree)
            .nodes
            .values()
            .filter(|node| node.tree_prefix == self.inner.site.key_prefix)
            .map(|node| node.info.options.id.clone())
            .collect();
        lock(&self.inner.anonymous).assign_reserved(&mut rows, &mut [], &existing);
        let old = std::mem::replace(&mut *lock(&self.inner.rows), rows.clone());
        let mut next = IndexMap::new();
        for row in rows {
            next.insert(row.id.clone(), row);
        }
        *lock(&self.inner.options) = next.clone();
        let ids: IndexSet<String> = old
            .into_iter()
            .map(|r| r.id)
            .chain(next.keys().cloned())
            .collect();
        let mut ids: Vec<_> = ids.into_iter().collect();
        // Reflect.ownKeys puts canonical array-index strings before other string keys.
        let index = |id: &str| {
            id.parse::<u32>()
                .ok()
                .filter(|n| *n != u32::MAX && n.to_string() == id)
        };
        ids.sort_by_key(|id| index(id).map_or((true, 0), |index| (false, index)));
        (
            *version,
            ids.into_iter()
                .map(|id| {
                    let row = next.shift_remove(&id);
                    (id, row)
                })
                .collect(),
        )
    }
    async fn run(&self, (version, plan): (u64, Vec<(String, Option<Entry>)>)) -> Vec<EntryChange> {
        join_all(plan.into_iter().map(|(id, row)| async move {
            let full = self.full_id(&id);
            let key = self.row_key(&id);
            let attempted = row.clone();
            let result = match row {
                Some(row) => self.upsert(row, Some(version)).await,
                None => {
                    self.remove_entry(&key, false, Some(version)).await;
                    Ok(EntryChange::Removed(full.clone()))
                }
            };
            match result {
                Ok(change) => {
                    lock(&self.inner.site.tree).changes.push(change.clone());
                    change
                }
                Err(error) => {
                    self.inner.ctx.logger().error(&error);
                    if let Some(node) = lock(&self.inner.site.tree).nodes.get_mut(&key)
                        && attempted.as_ref() == Some(&node.info.options)
                    {
                        node.update_error = Some(error.to_string());
                        if let LoadError::Kernel(error) = error {
                            node.cause = Some(error);
                        }
                    }
                    EntryChange::Updated(full, UpdateAction::None)
                }
            }
        }))
        .await
    }
    async fn upsert(&self, row: Entry, expected: Option<u64>) -> Result<EntryChange, LoadError> {
        let id = self.full_id(&row.id);
        let key = self.row_key(&row.id);
        let disabled = self.disabled(&row);
        let definition = self.inner.registry.resolve(&row.name).cloned();
        let mut ctx = self.inner.ctx.clone();
        if let Some(isolates) = &row.isolate {
            let mut tree = lock(&self.inner.site.tree);
            for (name, isolation) in isolates {
                let realm = match isolation {
                    Isolate::Private => Realm::new(),
                    Isolate::Label(label) => *tree
                        .labels
                        .entry((name.clone(), label.clone()))
                        .or_default(),
                };
                ctx = ctx.isolate(name, realm);
            }
        }
        if let Some(intercepts) = &row.intercept {
            for (name, value) in intercepts {
                ctx = ctx.intercept(name, value.clone());
            }
        }
        let site = Site {
            owner: Some(id.clone()),
            owner_key: Some(key.clone()),
            ..self.inner.site.clone()
        };
        ctx = ctx.with_tag(site);
        let mut dependencies = Vec::new();
        if let Some(def) = &definition {
            for dep in def.deps() {
                if !dependencies.iter().any(|n| n == dep.name) {
                    dependencies.push(dep.name.into());
                }
            }
        }
        for name in row.inject.iter().flatten() {
            if !dependencies.contains(name) {
                dependencies.push(name.clone());
            }
        }
        let (generation, old) = {
            let version = lock(&self.inner.version);
            if self.inner.closed.load(Ordering::Acquire)
                || expected.is_some_and(|expected| *version != expected)
                || lock(&self.inner.options).get(&row.id) != Some(&row)
            {
                return Ok(EntryChange::Updated(id, UpdateAction::None));
            }
            let mut tree = lock(&self.inner.site.tree);
            let old = tree.nodes.get(&key).cloned();
            tree.sequence += 1;
            let generation = tree.sequence;
            tree.nodes.insert(
                key.clone(),
                Node {
                    info: EntryInfo {
                        id: id.clone(),
                        name: row.name.clone(),
                        group: self.inner.site.owner.clone(),
                        disabled: disabled.as_ref().copied().unwrap_or(false),
                        options: row.clone(),
                        fiber: old.as_ref().and_then(|n| n.info.fiber.clone()),
                        status: if disabled.as_ref().copied().unwrap_or(false) {
                            EntryStatus::Disabled
                        } else if definition.is_none() {
                            EntryStatus::Unresolved
                        } else {
                            EntryStatus::Mounted
                        },
                    },
                    ctx: old
                        .as_ref()
                        .map(|n| n.ctx.clone())
                        .unwrap_or_else(|| ctx.clone()),
                    generation,
                    error: None,
                    update_error: None,
                    source: self.inner.site.base.clone(),
                    dependencies,
                    cause: None,
                    tree_prefix: self.inner.site.key_prefix.clone(),
                    owner: Arc::downgrade(&self.inner),
                },
            );
            (generation, old)
        };
        self.apply_row(row, id, key, disabled, definition, ctx, generation, old)
            .await
    }
    fn disabled(&self, row: &Entry) -> Result<bool, LoadError> {
        if row.group {
            return Ok(false);
        }
        let mut values = vec![row.disabled.clone()];
        {
            let tree = lock(&self.inner.site.tree);
            let mut owner = self.inner.site.owner_key.clone();
            let mut seen = IndexSet::new();
            while let Some(key) = owner {
                if !seen.insert(key.clone()) {
                    break;
                }
                let Some(node) = tree.nodes.get(&key) else {
                    break;
                };
                values.push(node.info.options.disabled.clone());
                owner = node
                    .owner
                    .upgrade()
                    .and_then(|group| group.site.owner_key.clone());
            }
        }
        for value in values {
            let disabled = if let Some(source) = expression(&value) {
                Expr::parse(source)
                    .and_then(|expr| {
                        expr.eval(
                            &self.inner.env,
                            &self.inner.ctx,
                            &self.inner.registry,
                            &self.inner.site.base,
                        )
                    })
                    .map(|v| v.as_ref().is_some_and(is_truthy))
                    .map_err(|e| LoadError::Runtime(format!("disabled expression failed: {e}")))
            } else {
                Ok(is_truthy(&value))
            }?;
            if disabled {
                return Ok(true);
            }
        }
        Ok(false)
    }
    #[allow(clippy::too_many_arguments)] // One row's already-published lifecycle plan.
    async fn apply_row(
        &self,
        row: Entry,
        id: String,
        key: String,
        disabled: Result<bool, LoadError>,
        definition: Option<hoh_kernel::PluginDef>,
        ctx: Context,
        generation: u64,
        old: Option<Node>,
    ) -> Result<EntryChange, LoadError> {
        let disabled = disabled?;
        if disabled {
            if let Some(fiber) = old.as_ref().and_then(|n| n.info.fiber.clone()) {
                fiber.dispose().await;
            }
            self.publish(&key, generation, None, None);
            return Ok(match old {
                None => EntryChange::Created(id),
                Some(old) => EntryChange::Updated(
                    id,
                    if old.info.disabled {
                        UpdateAction::None
                    } else {
                        UpdateAction::Disabled
                    },
                ),
            });
        }
        let context_changed = old.as_ref().is_some_and(|n| {
            n.info.options.name != row.name
                || n.info.options.inject != row.inject
                || n.info.options.isolate != row.isolate
                || n.info.options.intercept != row.intercept
                || !Weak::ptr_eq(&n.owner, &Arc::downgrade(&self.inner))
        });
        let existing = old
            .as_ref()
            .and_then(|n| n.info.fiber.clone())
            .filter(|f| !f.is_disposed());
        if context_changed && let Some(fiber) = &existing {
            lock(&self.inner.site.tree).disposing.insert(fiber.uid());
            fiber.dispose().await;
            lock(&self.inner.site.tree)
                .disposing
                .shift_remove(&fiber.uid());
        }
        if context_changed || existing.is_none() {
            if !self.current(&key, generation) {
                return Ok(EntryChange::Updated(id, UpdateAction::None));
            }
            let Some(definition) = definition else {
                let error = format!("cannot resolve plugin {}", row.name);
                self.inner.ctx.logger().error(&error);
                self.publish(&key, generation, None, Some(error));
                return Ok(if old.is_none() {
                    EntryChange::Created(id)
                } else {
                    EntryChange::Updated(id, UpdateAction::None)
                });
            };
            let fiber = ctx.plugin_dyn(
                &definition,
                row.config.clone(),
                PluginOptions {
                    entry_id: Some(Arc::from(id.as_str())),
                    inject: row
                        .inject
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .collect::<IndexSet<_>>()
                        .into_iter()
                        .filter(|name| !definition.deps().iter().any(|dep| dep.name == name))
                        .map(Arc::from)
                        .collect(),
                    resolver: (!self.inner.registry.is_carrier(&row.name)).then(|| {
                        Arc::new(Resolver {
                            env: self.inner.env.clone(),
                            registry: self.inner.registry.clone(),
                            base: self.inner.site.base.clone(),
                        }) as Arc<dyn ConfigResolver>
                    }),
                },
            )?;
            let published = {
                let mut tree = lock(&self.inner.site.tree);
                if let Some(node) = tree.nodes.get_mut(&key)
                    && node.generation == generation
                {
                    let old_uid = node.info.fiber.as_ref().map(Fiber::uid);
                    node.info.fiber = Some(fiber.clone());
                    node.error = None;
                    node.ctx = ctx;
                    if let Some(uid) = old_uid {
                        tree.fibres.shift_remove(&uid);
                    }
                    tree.fibres.insert(fiber.uid(), key.clone());
                    true
                } else {
                    false
                }
            };
            if !published {
                fiber.dispose().await;
            }
            return Ok(if old.is_none() {
                EntryChange::Created(id)
            } else {
                EntryChange::Updated(
                    id,
                    if context_changed && existing.is_some() {
                        UpdateAction::Rebuilt
                    } else {
                        UpdateAction::Enabled
                    },
                )
            });
        }
        let Some(fiber) = existing else {
            return Ok(EntryChange::Updated(id, UpdateAction::None));
        };
        let before = old
            .as_ref()
            .map(|n| &n.info.options.config)
            .unwrap_or(&Value::Null);
        let changed = !hoh_util_values::deep_equal_json(before, &row.config);
        let mut action = UpdateAction::None;
        let config_only = old.as_ref().is_some_and(|old| {
            let mut options = old.info.options.clone();
            options.config = row.config.clone();
            options == row
        });
        if changed
            && config_only
            && fiber.state() == FiberState::Active
            && definition.as_ref().is_some_and(|def| {
                hoh_kernel::config::equal_except_volatile(def.schema(), before, &row.config)
            })
        {
            // Retain invalid volatile candidates too, as the source rawConfig setter does.
            fiber.retain_raw_config(row.config.clone()).await?;
            match fiber.commit_volatile(row.config.clone()).await {
                Ok(true) => action = UpdateAction::Volatile,
                Ok(false) => {}
                Err(error) => {
                    self.inner
                        .ctx
                        .logger()
                        .warn(format!("volatile config update failed for {}", row.id));
                    self.inner.ctx.logger().warn(error);
                    action = UpdateAction::Volatile;
                }
            }
        }
        let failed = fiber.state() == FiberState::Failed;
        if action == UpdateAction::None
            && (changed
                || failed
                || row.group
                || definition
                    .as_ref()
                    .is_some_and(|definition| definition.name() == "cordis:group"))
        {
            fiber.update(row.config).await?;
            if changed || failed {
                action = UpdateAction::Config;
            }
        }
        Ok(EntryChange::Updated(id, action))
    }
    fn current(&self, id: &str, generation: u64) -> bool {
        lock(&self.inner.site.tree)
            .nodes
            .get(id)
            .is_some_and(|n| n.generation == generation)
    }
    fn publish(&self, id: &str, generation: u64, fiber: Option<Fiber>, error: Option<String>) {
        let mut tree = lock(&self.inner.site.tree);
        if let Some(node) = tree.nodes.get_mut(id)
            && node.generation == generation
        {
            let old_uid = node.info.fiber.as_ref().map(Fiber::uid);
            let new_uid = fiber.as_ref().map(Fiber::uid);
            node.info.fiber = fiber;
            node.error = error;
            if let Some(uid) = old_uid {
                tree.fibres.shift_remove(&uid);
            }
            if let Some(uid) = new_uid {
                tree.fibres.insert(uid, id.into());
            }
        }
    }
    async fn remove_node(&self, id: &str) {
        self.remove_entry(id, false, None).await;
    }
    async fn remove_entry(&self, id: &str, unlink: bool, expected: Option<u64>) {
        let node = {
            let version = lock(&self.inner.version);
            if expected.is_some_and(|expected| *version != expected) {
                return;
            }
            let mut tree = lock(&self.inner.site.tree);
            if tree
                .nodes
                .get(id)
                .is_some_and(|n| Weak::ptr_eq(&n.owner, &Arc::downgrade(&self.inner)))
            {
                if unlink {
                    let mut rows = lock(&self.inner.rows);
                    if let Some(index) = rows.iter().rposition(|row| self.row_key(&row.id) == id) {
                        let row = rows.remove(index);
                        lock(&self.inner.options).shift_remove(&row.id);
                    }
                }
                let node = tree.nodes.shift_remove(id);
                if let Some(uid) = node
                    .as_ref()
                    .and_then(|node| node.info.fiber.as_ref().map(Fiber::uid))
                {
                    tree.fibres.shift_remove(&uid);
                }
                node
            } else {
                None
            }
        };
        if let Some(fiber) = node.and_then(|n| n.info.fiber) {
            fiber.dispose().await;
        }
    }
    async fn stop(&self) {
        self.drain(self.close()).await;
    }
    fn close(&self) -> Vec<String> {
        {
            let mut version = lock(&self.inner.version);
            *version += 1;
            self.inner.closed.store(true, Ordering::Release);
            lock(&self.inner.options).clear();
            std::mem::take(&mut *lock(&self.inner.rows))
        }
        .into_iter()
        .map(|r| self.row_key(&r.id))
        .collect::<Vec<_>>()
    }
    async fn drain(&self, ids: Vec<String>) {
        join_all(ids.iter().map(|id| self.remove_node(id))).await;
        let mut tree = lock(&self.inner.site.tree);
        if tree
            .groups
            .get(&self.key())
            .and_then(Weak::upgrade)
            .is_some_and(|g| Arc::ptr_eq(&g, &self.inner))
        {
            tree.groups.shift_remove(&self.key());
        }
    }
}

pub(crate) struct GroupPlugin;
impl Plugin for GroupPlugin {
    const NAME: &'static str = "cordis:group";
    type Config = Vec<Entry>;
    type Deps = ();
    async fn apply(ctx: Context, _: (), config: Cfg<Vec<Entry>>) -> Result<(), Error> {
        let shared = ctx
            .tag::<Runtime>()
            .ok_or("group requires a Loader context")?;
        let group = Group::mount(&ctx, shared.registry.clone(), shared.env.clone())?;
        group.update((*config.get()).clone()).await;
        ctx.on_update::<Vec<Entry>, _>(move |config, _| {
            let group = group.clone();
            async move {
                group.update((*config).clone()).await;
                Ok(())
            }
        })?;
        Ok(())
    }
}

fn rows(value: &Value, file: &Path) -> Result<Vec<Entry>, LoadError> {
    parse_entries(&crate::yaml::write_yaml(value), file)
}
fn read_entries(text: &str, file: &Path) -> Result<Vec<Entry>, LoadError> {
    match file.extension().and_then(|extension| extension.to_str()) {
        Some("yml" | "yaml") => parse_entries(text, file),
        Some("json") => {
            let value = hoh_util_values::js_json_parse(text).map_err(|error| LoadError::Shape {
                file: file.into(),
                line: 1,
                column: 1,
                message: error.to_string(),
            })?;
            rows(&value, file)
        }
        extension => Err(LoadError::Runtime(format!(
            "extension \"{}\" not supported",
            extension
                .map(|value| format!(".{value}"))
                .unwrap_or_default()
        ))),
    }
}
fn include_entries(text: &str, file: &Path) -> Result<Vec<Entry>, LoadError> {
    read_entries(text, file).map_err(|mut source| {
        let stage = match &mut source {
            LoadError::Yaml { line, message, .. } => {
                if *line > text.lines().count() && message.contains("flow") {
                    *message = "unexpected end of the stream within a flow collection".into();
                }
                "parse"
            }
            LoadError::Shape { message, .. } if message.contains("top-level YAML array") => {
                source = LoadError::Runtime("config file must be a top-level array".into());
                "validate"
            }
            LoadError::Shape { .. } => "validate",
            _ => "parse",
        };
        LoadError::ConfigFile {
            stage,
            file: file.into(),
            source: Box::new(source),
        }
    })
}
fn assign_initial_ids(raw: &mut Value, assigned: &[Entry]) {
    for (row, entry) in raw.as_array_mut().into_iter().flatten().zip(assigned) {
        if let Some(map) = row.as_object_mut() {
            map.insert("id".into(), entry.id.clone().into());
            if entry.group
                && let Some(children) = map.get_mut("config")
                && let Ok(assigned) = serde_json::from_value::<Vec<Entry>>(entry.config.clone())
            {
                assign_initial_ids(children, &assigned);
            }
        }
    }
}
pub(crate) fn patches_value(patches: &[crate::Patch]) -> Value {
    Value::Array(
        patches
            .iter()
            .map(|p| {
                let mut value = serde_json::Map::new();
                if let Some(id) = &p.id {
                    value.insert("id".into(), id.clone());
                }
                if let Some(name) = &p.name {
                    value.insert("name".into(), name.clone().into());
                }
                value.extend(p.set.clone());
                if let Some(insert) = &p.insert {
                    value.insert("insert".into(), crate::yaml::entries_value(insert));
                }
                Value::Object(value)
            })
            .collect(),
    )
}

pub(crate) struct IncludePlugin;
#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct IncludeConfig {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    initial: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    patches: Option<Vec<Value>>,
    #[serde(
        default,
        rename = "enableLogs",
        skip_serializing_if = "Option::is_none"
    )]
    enable_logs: Option<bool>,
}
impl Plugin for IncludePlugin {
    const NAME: &'static str = "cordis:include";
    type Config = IncludeConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), config: Cfg<IncludeConfig>) -> Result<(), Error> {
        let shared = ctx
            .tag::<Runtime>()
            .ok_or("include requires a Loader context")?;
        let site = ctx.tag::<Site>().ok_or("include requires an entry")?;
        let include = IncludeState {
            ctx: ctx.clone(),
            runtime: shared.clone(),
            parent: (*site).clone(),
            current: Mutex::new(None),
            generation: Mutex::new(0),
        };
        let owner = site.owner_key.clone().unwrap_or_default();
        let tree = Arc::downgrade(&site.tree);
        ctx.effect("include cache", move || {
            Undo::sync(move || {
                if let Some(tree) = tree.upgrade() {
                    let mut tree = lock(&tree);
                    tree.bases.shift_remove(&owner);
                    tree.identities
                        .retain(|(candidate, _), _| candidate != &owner);
                }
            })
        })?;
        let include = Arc::new(include);
        include
            .apply(&serde_json::to_value(config.get().as_ref())?, false)
            .await?;
        ctx.on_update::<IncludeConfig, _>(move |config, _| {
            let include = include.clone();
            async move {
                let owner = include.parent.owner_key.clone().unwrap_or_default();
                let refresh = lock(&include.parent.tree).refreshes.shift_remove(&owner);
                include
                    .apply(&serde_json::to_value(config.as_ref())?, refresh)
                    .await
            }
        })?;
        Ok(())
    }
}
struct IncludeState {
    ctx: Context,
    runtime: Arc<Runtime>,
    parent: Site,
    current: Mutex<Option<(PathBuf, Group)>>,
    generation: Mutex<u64>,
}
impl IncludeState {
    async fn apply(&self, config: &Value, refresh: bool) -> Result<(), Error> {
        let generation = {
            let mut current = lock(&self.generation);
            *current += 1;
            *current
        };
        let path = config["path"]
            .as_str()
            .ok_or("include path must be a string")?;
        let path = if path.starts_with("file:") {
            url::Url::parse(path)?
                .to_file_path()
                .map_err(|_| "invalid include file URL")?
        } else {
            self.parent.base.join(path)
        };
        let path = PathBuf::from(if cfg!(windows) {
            crate::expr::windows_normalize(&path.to_string_lossy())
        } else {
            crate::expr::normalize(&path.to_string_lossy())
        });
        let owner = self.parent.owner_key.clone().unwrap_or_default();
        let cached = if !refresh
            && lock(&self.current)
                .as_ref()
                .is_some_and(|(old, _)| old == &path)
        {
            lock(&self.parent.tree).bases.get(&owner).cloned()
        } else {
            None
        };
        let canonical = if cached.is_some() {
            lock(&self.current)
                .as_ref()
                .and_then(|(_, group)| group.inner.site.includes.last())
                .cloned()
                .unwrap_or_else(|| path.clone())
        } else {
            tokio::fs::canonicalize(&path)
                .await
                .unwrap_or_else(|_| path.clone())
        };
        if self.parent.includes.contains(&canonical) {
            return Err(format!("include cycle: {}", path.display()).into());
        }
        let mut base = if let Some(cached) = cached {
            cached
        } else {
            let text = match tokio::fs::read_to_string(&path).await {
                Ok(text) => text,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && config.get("initial").is_some_and(is_truthy) =>
                {
                    let mut initial = config["initial"].clone();
                    let mut assigned = rows(&initial, &path)?;
                    lock(&self.parent.tree)
                        .identities
                        .entry((owner.clone(), path.clone()))
                        .or_default()
                        .assign(&mut assigned, &mut []);
                    assign_initial_ids(&mut initial, &assigned);
                    let text = if path
                        .extension()
                        .is_some_and(|extension| extension == "json")
                    {
                        hoh_util_values::js_json_stringify_pretty(&initial, 2)
                    } else {
                        crate::yaml::write_yaml(&initial)
                    };
                    read_entries(&text, &path)?;
                    tokio::fs::write(&path, &text).await?;
                    text
                }
                Err(error) => {
                    let error = if error.kind() == std::io::ErrorKind::NotFound {
                        LoadError::Runtime(format!("config file not found: {}", path.display()))
                    } else {
                        LoadError::ConfigFile {
                            stage: "read",
                            file: path.clone(),
                            source: Box::new(LoadError::Io {
                                file: path.clone(),
                                source: error,
                            }),
                        }
                    };
                    return self.read_error(error, refresh);
                }
            };
            match include_entries(&text, &path) {
                Ok(base) => base,
                Err(error) => return self.read_error(error, refresh),
            }
        };
        let mut patches = parse_patches(
            &crate::yaml::write_yaml(config.get("patches").unwrap_or(&json!([]))),
            &path,
        )?;
        let previous = {
            let current = lock(&self.generation);
            if *current != generation {
                return Ok(());
            }
            lock(&self.current).clone()
        };
        let stopped = {
            let current = lock(&self.generation);
            if *current != generation {
                return Ok(());
            }
            previous
                .as_ref()
                .filter(|(old_path, _)| old_path != &path)
                .map(|(_, group)| (group.clone(), group.close()))
        };
        if let Some((group, ids)) = stopped {
            group.drain(ids).await;
        }
        let (group, applied) = {
            let current = lock(&self.generation);
            if *current != generation {
                return Ok(());
            }
            self.ctx.ensure_active()?;
            let identity = (
                self.parent.owner_key.clone().unwrap_or_default(),
                path.clone(),
            );
            {
                let mut tree = lock(&self.parent.tree);
                tree.identities
                    .entry(identity)
                    .or_default()
                    .assign_patches(&mut base, &mut patches);
                tree.bases.insert(
                    self.parent.owner_key.clone().unwrap_or_default(),
                    base.clone(),
                );
            }
            let root_warnings = self.parent.owner.as_deref() == Some("include")
                && self.ctx.get::<LoaderKey>().is_some_and(|loader| {
                    Arc::ptr_eq(&loader.root.inner.site.tree, &self.parent.tree)
                });
            let applied = crate::apply_patches(&base, &patches, &mut |warning| {
                if !root_warnings {
                    self.ctx.logger().warn(warning);
                }
            });
            let group = if let Some((old_path, group)) = &previous
                && old_path == &path
                && !group.inner.closed.load(Ordering::Acquire)
            {
                group.clone()
            } else {
                let mut site = self.parent.clone();
                site.base = path.parent().unwrap_or(Path::new(".")).into();
                site.prefix = site
                    .owner
                    .as_ref()
                    .map(|id| format!("{id}:"))
                    .unwrap_or_default();
                site.key_prefix = site
                    .owner_key
                    .as_ref()
                    .map(|key| format!("{key}:"))
                    .unwrap_or_default();
                site.includes.push(canonical);
                Group::at(
                    &self.ctx,
                    self.runtime.registry.clone(),
                    self.runtime.env.clone(),
                    site,
                )?
            };
            *lock(&self.current) = Some((path, group.clone()));
            (group, applied)
        };
        group
            .update_guarded(applied, &self.generation, generation)
            .await;
        Ok(())
    }
    fn read_error(&self, error: LoadError, refresh: bool) -> Result<(), Error> {
        if refresh && lock(&self.current).is_some() {
            self.ctx.logger().warn(error);
            Ok(())
        } else {
            Err(Box::new(error))
        }
    }
}

struct Runtime {
    registry: Arc<PluginRegistry>,
    env: EvalEnv,
}
struct LoaderState {
    composition: Composition,
    layers: Vec<Layer>,
}
pub struct Loader {
    root: Group,
    base: PathBuf,
    runtime: Arc<Runtime>,
    state: Mutex<LoaderState>,
}
impl Loader {
    pub fn entries(&self) -> Vec<EntryInfo> {
        self.root.entries()
    }
    pub(crate) fn records(&self) -> Vec<Node> {
        self.root.records()
    }
    pub(crate) async fn provision_store(&self) -> hoh_kernel::Snapshot {
        self.root.inner.ctx.reflect().store().await
    }
    pub(crate) fn failure(&self, id: &str) -> Option<String> {
        lock(&self.root.inner.site.tree)
            .nodes
            .get(&addressed_key(id))
            .and_then(Node::failure)
    }
    pub fn resolve(&self, id: &str) -> Option<EntryInfo> {
        // A colon denotes a subtree, not part of a tree-local selector (V27).
        let mut prefix = String::new();
        let parts: Vec<_> = id.split(':').collect();
        let tree = lock(&self.root.inner.site.tree);
        for (index, part) in parts.iter().enumerate() {
            prefix.push_str(&component(part));
            let node = tree.nodes.get(&prefix)?;
            if index + 1 == parts.len() {
                return Some(node.info.clone());
            }
            if self.runtime.registry.resolve(&node.info.name)?.name() != "cordis:include" {
                return None;
            }
            prefix.push(':');
        }
        None
    }
    fn group(&self, id: Option<&str>) -> Result<Group, LoadError> {
        match id {
            None => Ok(self.root.clone()),
            Some(id) => {
                self.resolve(id)
                    .ok_or_else(|| LoadError::Runtime(format!("cannot resolve entry {id}")))?;
                lock(&self.root.inner.site.tree)
                    .groups
                    .get(&addressed_key(id))
                    .and_then(Weak::upgrade)
                    .map(|inner| Group { inner })
                    .ok_or_else(|| LoadError::Runtime(format!("entry {id} is not a group")))
            }
        }
    }
    pub async fn create(&self, mut entry: Entry, group: Option<&str>) -> Result<String, LoadError> {
        let group = self.group(group)?;
        group.inner.ctx.ensure_active()?;
        let version = {
            let version = lock(&group.inner.version);
            if group.inner.closed.load(Ordering::Acquire) {
                return Err(KernelError::Inactive.into());
            }
            let mut rows = lock(&group.inner.rows);
            if entry.id.is_empty() {
                let mut tree = lock(&group.inner.site.tree);
                loop {
                    tree.sequence += 1;
                    let id = format!("{:08x}", tree.sequence);
                    if !tree.nodes.contains_key(&group.row_key(&id))
                        && !rows.iter().any(|r| r.id == id)
                    {
                        entry.id = id;
                        break;
                    }
                }
            }
            rows.push(entry.clone());
            lock(&group.inner.options).insert(entry.id.clone(), entry.clone());
            *version
        };
        let id = group.full_id(&entry.id);
        let change = group.upsert(entry, Some(version)).await?;
        lock(&group.inner.site.tree).changes.push(change);
        Ok(id)
    }
    pub async fn remove(&self, id: &str) -> Result<(), LoadError> {
        self.resolve(id)
            .ok_or_else(|| LoadError::Runtime(format!("cannot resolve entry {id}")))?;
        let key = addressed_key(id);
        let group = lock(&self.root.inner.site.tree)
            .nodes
            .get(&key)
            .and_then(|node| node.owner.upgrade())
            .map(|inner| Group { inner })
            .ok_or_else(|| LoadError::Runtime(format!("cannot resolve entry {id}")))?;
        group.remove_entry(&key, true, None).await;
        Ok(())
    }
    pub async fn settled(&self) {
        loop {
            let entries = self.entries();
            let before: Vec<_> = entries
                .iter()
                .filter_map(|e| e.fiber.as_ref().map(|f| (f.uid(), f.state())))
                .collect();
            join_all(
                entries
                    .into_iter()
                    .filter_map(|e| e.fiber)
                    .map(|fiber| async move {
                        let cause = fiber.settled().await.err();
                        let error = cause
                            .as_ref()
                            .map(|e| crate::startup_diagnostics::format_activation_error(e));
                        let mut tree = lock(&self.root.inner.site.tree);
                        if let Some(key) = tree.fibres.get(&fiber.uid()).cloned()
                            && let Some(node) = tree.nodes.get_mut(&key)
                            && node
                                .info
                                .fiber
                                .as_ref()
                                .is_some_and(|f| f.uid() == fiber.uid())
                        {
                            node.error = error;
                            if cause.is_some() {
                                node.cause = cause;
                            }
                        }
                    }),
            )
            .await;
            let after: Vec<_> = self
                .entries()
                .iter()
                .filter_map(|e| e.fiber.as_ref().map(|f| (f.uid(), f.state())))
                .collect();
            if before == after
                && after
                    .iter()
                    .all(|(_, state)| !matches!(state, FiberState::Loading | FiberState::Unloading))
            {
                break;
            }
        }
    }
    pub async fn recompose(&self, mut layers: Vec<Layer>) -> Result<Vec<EntryChange>, LoadError> {
        let root = self
            .resolve("include")
            .and_then(|e| e.fiber)
            .ok_or_else(|| {
                LoadError::Runtime("hoh: profile reload requires the root Include entry".into())
            })?;
        let mut base = lock(&self.root.inner.site.tree)
            .bases
            .get("include")
            .cloned()
            .unwrap_or_default();
        lock(&self.root.inner.site.tree)
            .identities
            .entry(("include".into(), self.base.clone()))
            .or_default()
            .assign(&mut base, &mut layers);
        let composition = compose(&base, &layers, &mut |warning| {
            self.root.inner.ctx.logger().warn(warning)
        });
        let previous = self.entries();
        lock(&self.root.inner.site.tree).changes.clear();
        let patches: Vec<_> = layers
            .iter()
            .flat_map(|l| l.patches.iter().cloned())
            .collect();
        let config =
            json!({"path": self.base.to_string_lossy(), "patches": patches_value(&patches)});
        {
            let mut state = lock(&self.state);
            state.composition = composition;
            state.layers = layers;
        }
        if let Some(node) = lock(&self.root.inner.site.tree).nodes.get_mut("include") {
            node.info.options.config = config.clone();
        }
        root.update(config).await?;
        self.settled().await;
        let mut introduced = vec![];
        for node in self.records() {
            let entry = &node.info;
            if entry.disabled {
                continue;
            }
            let failure = node.failure();
            if let Some(failure) = failure
                && entry
                    .fiber
                    .as_ref()
                    .is_none_or(|f| f.state() != FiberState::Pending)
            {
                if previous.iter().any(|old| {
                    old.id == entry.id
                        && old.options == entry.options
                        && old.fiber.as_ref().map(Fiber::uid)
                            == entry.fiber.as_ref().map(Fiber::uid)
                }) {
                    self.root.inner.ctx.logger().warn(&failure);
                } else {
                    introduced.push(failure);
                }
            }
        }
        if !introduced.is_empty() {
            return Err(LoadError::Runtime(format!(
                "hoh: warning: {} {} did not activate\n{}",
                introduced.len(),
                if introduced.len() == 1 {
                    "entry"
                } else {
                    "entries"
                },
                introduced.join("\n")
            )));
        }
        self.root.inner.ctx.emit(&ConfigReload);
        Ok(std::mem::take(
            &mut lock(&self.root.inner.site.tree).changes,
        ))
    }
}
impl Node {
    pub(crate) fn failure(&self) -> Option<String> {
        let node = self;
        if node.info.disabled {
            return None;
        }
        if let Some(error) = node.update_error.as_ref().or(node.error.as_ref()) {
            let phase =
                if node.update_error.is_none() && node.info.status == EntryStatus::Unresolved {
                    "failed to import: "
                } else {
                    ""
                };
            return Some(format!(
                "{} ({}): {phase}{error}",
                node.info.options.id, node.info.name
            ));
        }
        let fiber = node.info.fiber.as_ref()?;
        if fiber.state() == FiberState::Active {
            return None;
        }
        let missing = node
            .dependencies
            .iter()
            .filter(|name| !node.ctx.has(name))
            .cloned()
            .collect::<Vec<_>>();
        Some(format!(
            "{} ({}): pending (waiting for {}: {})",
            node.info.options.id,
            node.info.name,
            if missing.len() == 1 {
                "service"
            } else {
                "services"
            },
            if missing.is_empty() {
                "unknown".into()
            } else {
                missing.join(", ")
            }
        ))
    }
    pub(crate) fn cause(&self) -> Option<KernelError> {
        self.cause.clone().or_else(|| {
            self.update_error
                .as_ref()
                .or(self.error.as_ref())
                .map(|error| KernelError::Failed(Arc::new(error.clone().into())))
        })
    }
}
impl Loader {
    pub fn audit(&self, required: &[&str]) -> Audit {
        let entries = self.entries();
        let missing = required
            .iter()
            .filter(|id| !entries.iter().any(|e| e.id == **id || e.options.id == **id))
            .map(|id| (*id).into())
            .collect();
        let unresolved = entries
            .iter()
            .filter(|e| e.status == EntryStatus::Unresolved)
            .map(|e| e.id.clone())
            .collect();
        let inactive = self
            .records()
            .into_iter()
            .filter(|node| !node.info.disabled)
            .filter_map(|node| {
                let entry = &node.info;
                let fiber = entry.fiber.as_ref()?;
                if fiber.state() == FiberState::Active {
                    return None;
                }
                Some(FiberInfo {
                    uid: fiber.uid(),
                    parent: node.ctx.fiber().uid(),
                    name: fiber.name(),
                    entry_id: fiber.entry_id(),
                    state: fiber.state(),
                    missing: node
                        .dependencies
                        .iter()
                        .filter(|name| !node.ctx.has(name))
                        .map(|s| Arc::from(s.as_str()))
                        .collect(),
                    error: node.error.clone(),
                })
            })
            .collect();
        Audit {
            missing,
            unresolved,
            inactive,
        }
    }
    pub fn dump(&self) -> String {
        render_dump(&lock(&self.state).composition)
    }
    /// Re-read an Include. Invalid file edits warn and retain its last good children.
    pub async fn refresh(&self, id: &str) -> Result<(), LoadError> {
        let fiber = self
            .resolve(id)
            .and_then(|e| e.fiber)
            .ok_or_else(|| LoadError::Runtime(format!("cannot resolve entry {id}")))?;
        if fiber.name().as_ref() != "cordis:include" {
            return Err(LoadError::Runtime(format!("entry {id} is not an include")));
        }
        lock(&self.root.inner.site.tree)
            .refreshes
            .insert(addressed_key(id));
        fiber.update((*fiber.raw_config()).clone()).await?;
        self.settled().await;
        if id == "include"
            && let Some(base) = lock(&self.root.inner.site.tree)
                .bases
                .get("include")
                .cloned()
        {
            let mut state = lock(&self.state);
            state.composition = compose(&base, &state.layers, &mut |warning| {
                self.root.inner.ctx.logger().warn(warning)
            });
        }
        Ok(())
    }
    pub(crate) fn profile_entries(&self) -> Vec<EntryInfo> {
        self.records()
            .into_iter()
            .filter(|node| node.tree_prefix == "include:")
            .map(|node| node.info)
            .collect()
    }
    pub(crate) fn realm(&self, id: &str, service: &str) -> Option<Realm> {
        lock(&self.root.inner.site.tree)
            .nodes
            .get(&addressed_key(id))
            .map(|n| n.ctx.realm_of(service))
    }
}

pub async fn boot(root: &Context, options: BootOptions) -> Result<Service<LoaderKey>, LoadError> {
    let mut result = boot_inner(root, options).await;
    if let Err(error) = &mut result {
        root.fiber().dispose().await;
        if let LoadError::Startup(error) = error {
            error.messages = root
                .logger()
                .buffer()
                .iter()
                .filter(|record| {
                    matches!(
                        record.level,
                        hoh_kernel::Level::Warn | hoh_kernel::Level::Error
                    )
                })
                .map(|record| record.message.clone())
                .collect();
        } else {
            return result.map_err(|error| LoadError::Boot(Box::new(error)));
        }
    }
    result
}
async fn boot_inner(
    root: &Context,
    mut options: BootOptions,
) -> Result<Service<LoaderKey>, LoadError> {
    if !options.base.is_absolute() {
        options.base = options.env.cwd.join(&options.base);
    }
    let composition = Composition {
        entries: vec![],
        origins: IndexMap::new(),
    };
    let runtime = Arc::new(Runtime {
        registry: options.registry,
        env: options.env,
    });
    let ctx = root.with_tag(Runtime {
        registry: runtime.registry.clone(),
        env: runtime.env.clone(),
    });
    let site = Site {
        tree: Arc::new(Mutex::new(TreeState::default())),
        owner: None,
        prefix: String::new(),
        owner_key: None,
        key_prefix: String::new(),
        base: options.base.parent().unwrap_or(Path::new(".")).into(),
        includes: vec![],
    };
    let group = Group::at(&ctx, runtime.registry.clone(), runtime.env.clone(), site)?;
    let loader = Arc::new(Loader {
        root: group,
        base: options.base.clone(),
        runtime,
        state: Mutex::new(LoaderState {
            composition,
            layers: options.layers.clone(),
        }),
    });
    let provision = root.provide::<LoaderKey>(loader.clone()).await?;
    let patches: Vec<_> = options
        .layers
        .iter()
        .flat_map(|l| l.patches.iter().cloned())
        .collect();
    loader.root.update(vec![Entry { id: "include".into(), name: "cordis:include".into(), config: json!({"path": options.base.to_string_lossy(), "patches": patches_value(&patches)}), disabled: Value::Null, group: false, inject: None, isolate: None, intercept: None }]).await;
    loader.settled().await;
    let mut base = lock(&loader.root.inner.site.tree)
        .bases
        .get("include")
        .cloned()
        .unwrap_or_default();
    lock(&loader.root.inner.site.tree)
        .identities
        .entry(("include".into(), options.base.clone()))
        .or_default()
        .assign(&mut base, &mut options.layers);
    let composition = compose(&base, &options.layers, &mut |warning| {
        root.logger().warn(warning)
    });
    {
        let mut state = lock(&loader.state);
        state.composition = composition;
        state.layers = options.layers;
    }
    let result =
        crate::startup_diagnostics::audit_startup_entries(&loader, "hoh", &mut |warning| {
            root.logger().warn(warning)
        })
        .await;
    if let Err(error) = result {
        loader.root.stop().await;
        provision.dispose().await;
        return Err(error);
    }
    root.get::<LoaderKey>().ok_or(KernelError::Inactive.into())
}
