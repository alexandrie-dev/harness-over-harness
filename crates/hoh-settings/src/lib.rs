//! Schema-derived live configuration forms backed by the active profile editor.

mod redact;
mod schema;
mod types;
mod yaml;

pub use redact::{RedactedSecret, RedactedValue, redact_secrets};
pub use schema::{is_volatile_path, plain_config, project_form, volatile_form};
pub use types::*;

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Fiber, FiberState, Inject, NoConfig, Plugin, Service, Undo};
use hoh_loader::{
    ConfigReload, LoaderKey, ProfileContextKey,
    config_editor::{ConfigEditorKey, Configuration},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::sync::mpsc;

pub type SettingsFuture<T> = Pin<Box<dyn Future<Output = Result<T, BoxError>> + Send + 'static>>;
pub type SettingsUndo = Box<dyn Fn() + Send + Sync + 'static>;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDescriptor {
    pub ns: SettingsNamespace,
    pub auto_generate: bool,
    pub schema: Value,
    pub value: Value,
    pub revision: u64,
    pub base: Value,
    pub user: Value,
    pub applies: SettingsApplyMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets: Option<Vec<SettingsSecretView>>,
}

impl fmt::Debug for SettingsDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SettingsDescriptor")
            .field("ns", &self.ns)
            .field("auto_generate", &self.auto_generate)
            .field("schema", &"<omitted>")
            .field("value", &"<redacted>")
            .field("revision", &self.revision)
            .field("base", &"<redacted>")
            .field("user", &"<redacted>")
            .field("applies", &self.applies)
            .field("secrets", &self.secrets)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingsDescribeOptions {
    pub redact_secrets: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SettingsPathOp {
    Set { path: Vec<String>, value: Value },
    Unset { path: Vec<String> },
}
impl SettingsPathOp {
    pub fn path(&self) -> &[String] {
        match self {
            Self::Set { path, .. } | Self::Unset { path } => path,
        }
    }
}
impl From<SettingsPathOpView> for SettingsPathOp {
    fn from(value: SettingsPathOpView) -> Self {
        match value {
            SettingsPathOpView::Set { path, value } => Self::Set { path, value },
            SettingsPathOpView::Unset { path } => Self::Unset { path },
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error(
    "settings namespace \"{namespace}\" changed since it was read (expected revision {expected}, now {actual})"
)]
pub struct SettingsConflictError {
    pub namespace: String,
    pub expected: u64,
    pub actual: u64,
}
impl SettingsConflictError {
    pub const CODE: &'static str = "SETTINGS_CONFLICT";
}
impl SettingsConflictError {
    pub fn code(&self) -> &'static str {
        Self::CODE
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsPresentation {
    pub auto: Option<bool>,
}

pub trait SettingsApi: Send + Sync + 'static {
    fn configure(
        &self,
        presentation: SettingsPresentation,
        owner: Fiber,
    ) -> Result<SettingsUndo, BoxError>;
    fn writable(&self) -> bool;
    fn document_path(&self) -> PathBuf;
    fn prepare_document(&self) -> SettingsFuture<PathBuf>;
    fn describe(&self, options: SettingsDescribeOptions)
    -> SettingsFuture<Vec<SettingsDescriptor>>;
    fn update(&self, ns: String, patch: Value, expected: Option<u64>) -> SettingsFuture<()>;
    fn replace(&self, ns: String, section: Value, expected: Option<u64>) -> SettingsFuture<()>;
    fn mutate(
        &self,
        ns: String,
        ops: Vec<SettingsPathOp>,
        expected: Option<u64>,
    ) -> SettingsFuture<()>;
}
hoh_kernel::service!(pub Settings: dyn SettingsApi = "settings");

#[derive(Clone, Debug)]
pub struct SettingsDocumentUpdated {
    pub ns: SettingsNamespace,
    pub revision: u64,
}
hoh_kernel::event!(
    SettingsDocumentUpdated = "settings/document-updated",
    hoh_kernel::Emit
);

#[derive(Clone)]
struct Revision {
    raw: Option<String>,
    revision: u64,
    ns: SettingsNamespace,
    auto_generate: bool,
}
#[derive(Clone)]
struct Presentation {
    token: u64,
    auto: Option<bool>,
}
struct Inner {
    ctx: Context,
    editor: Service<ConfigEditorKey>,
    revisions: Mutex<indexmap::IndexMap<String, Revision>>,
    presentations: Mutex<HashMap<u32, Presentation>>,
    presentation_sequence: AtomicU64,
    closed: AtomicBool,
    scheduled: AtomicBool,
    refresh: mpsc::UnboundedSender<()>,
}
#[derive(Clone)]
pub struct SettingsForms(Arc<Inner>);
enum WriteChange {
    Update(Value),
    Replace(Value),
    Mutate(Vec<SettingsPathOp>),
}

impl SettingsForms {
    fn create(
        ctx: Context,
        editor: Service<ConfigEditorKey>,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<()>) {
        let (refresh, receiver) = mpsc::unbounded_channel();
        (
            Arc::new(Self(Arc::new(Inner {
                ctx,
                editor,
                revisions: Mutex::new(indexmap::IndexMap::new()),
                presentations: Mutex::new(HashMap::new()),
                presentation_sequence: AtomicU64::new(0),
                closed: AtomicBool::new(false),
                scheduled: AtomicBool::new(false),
                refresh,
            }))),
            receiver,
        )
    }
    fn invalidate(&self) {
        if self.0.closed.load(Ordering::Acquire) || self.0.scheduled.swap(true, Ordering::AcqRel) {
            return;
        }
        if self.0.refresh.send(()).is_err() {
            self.0.scheduled.store(false, Ordering::Release);
        }
    }
    pub fn writable(&self) -> bool {
        true
    }
    pub fn document_path(&self) -> PathBuf {
        self.0.editor.document_path()
    }
    pub async fn prepare_document(&self) -> Result<PathBuf, BoxError> {
        Ok(self.document_path())
    }

    pub fn configure(
        &self,
        presentation: SettingsPresentation,
        owner: Fiber,
    ) -> Result<SettingsUndo, BoxError> {
        let uid = owner.uid();
        let token = self
            .0
            .presentation_sequence
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        {
            let mut presentations = lock(&self.0.presentations);
            if presentations.contains_key(&uid) {
                return Err(
                    "Settings presentation is already configured for this plugin instance".into(),
                );
            }
            presentations.insert(
                uid,
                Presentation {
                    token,
                    auto: presentation.auto,
                },
            );
        }
        self.invalidate();
        let weak = Arc::downgrade(&self.0);
        Ok(Box::new(move || {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let removed = {
                let mut presentations = lock(&inner.presentations);
                if presentations
                    .get(&uid)
                    .is_some_and(|policy| policy.token == token)
                {
                    presentations.remove(&uid);
                    true
                } else {
                    false
                }
            };
            if removed {
                SettingsForms(inner).invalidate();
            }
        }))
    }

    pub async fn describe(
        &self,
        options: SettingsDescribeOptions,
    ) -> Result<Vec<SettingsDescriptor>, BoxError> {
        let rows = self.0.editor.configuration().await?;
        let mut active = std::collections::HashSet::new();
        let mut descriptors = vec![];
        for row in rows {
            if let Some(descriptor) = self.describe_row(&row, options, &mut active).await? {
                descriptors.push(descriptor);
            }
        }
        let removed = {
            let mut revisions = lock(&self.0.revisions);
            let mut removed = vec![];
            for (id, previous) in revisions.iter_mut() {
                if active.contains(id) || previous.raw.is_none() {
                    continue;
                }
                previous.revision += 1;
                previous.raw = None;
                removed.push((previous.ns.clone(), previous.revision));
            }
            removed
        };
        for (ns, revision) in removed {
            self.0.ctx.emit(&SettingsDocumentUpdated { ns, revision });
        }
        Ok(descriptors)
    }

    async fn describe_row(
        &self,
        row: &Configuration,
        options: SettingsDescribeOptions,
        active: &mut std::collections::HashSet<String>,
    ) -> Result<Option<SettingsDescriptor>, BoxError> {
        let Some(fiber) = row.entry.fiber.as_ref() else {
            return Ok(None);
        };
        if fiber.state() != FiberState::Active {
            return Ok(None);
        }
        let Some(schema) = self.0.editor.schema(&row.entry)? else {
            return Ok(None);
        };
        let Some(form) = volatile_form(&schema) else {
            return Ok(None);
        };
        let Some(effective) = fiber.effective_config() else {
            return Ok(None);
        };
        active.insert(row.entry.id.clone());
        let entry_config = if row.entry.options.config.is_null() {
            json!({})
        } else {
            row.entry.options.config.clone()
        };
        let raw = serde_json::to_string(&json!([fiber.uid(), schema, entry_config]))?;
        let auto_generate = lock(&self.0.presentations)
            .get(&fiber.uid())
            .and_then(|policy| policy.auto)
            .unwrap_or(true);
        let (revision, changed) = {
            let mut revisions = lock(&self.0.revisions);
            let previous = revisions.get(&row.entry.id).cloned();
            let revision = previous.as_ref().map_or(0, |previous| {
                previous.revision + u64::from(previous.raw.as_deref() != Some(raw.as_str()))
            });
            let changed = previous.as_ref().is_none_or(|previous| {
                previous.raw.as_deref() != Some(raw.as_str())
                    || previous.auto_generate != auto_generate
            });
            revisions.insert(
                row.entry.id.clone(),
                Revision {
                    raw: Some(raw),
                    revision,
                    ns: row.entry.options.id.clone().into(),
                    auto_generate,
                },
            );
            (revision, changed)
        };
        if changed {
            self.0.ctx.emit(&SettingsDocumentUpdated {
                ns: row.entry.options.id.clone().into(),
                revision,
            });
        }
        let value = project_form(&form, &plain_config(&effective));
        let inherited = self
            .0
            .editor
            .resolve_inherited(&row.entry, &row.inherited)
            .await?;
        let base = project_form(&form, &plain_config(&inherited));
        let user = project_form(&form, &row.r#override);
        let redacted = redact_secrets(&form, &value);
        let (value, base, user, secrets) = if options.redact_secrets {
            (
                redacted.value,
                redact_secrets(&form, &base).value,
                redact_secrets(&form, &user).value,
                Some(redacted.secrets),
            )
        } else {
            (value, base, user, None)
        };
        Ok(Some(SettingsDescriptor {
            ns: row.entry.options.id.clone().into(),
            auto_generate,
            schema: form,
            value,
            revision,
            base,
            user,
            applies: SettingsApplyMode::Live,
            secrets,
        }))
    }

    pub async fn update(
        &self,
        ns: impl Into<String>,
        patch: Value,
        expected: Option<u64>,
    ) -> Result<(), BoxError> {
        require_object(&patch)?;
        self.write(ns.into(), WriteChange::Update(patch), expected)
            .await
    }
    pub async fn replace(
        &self,
        ns: impl Into<String>,
        section: Value,
        expected: Option<u64>,
    ) -> Result<(), BoxError> {
        require_object(&section)?;
        self.write(ns.into(), WriteChange::Replace(section), expected)
            .await
    }
    pub async fn mutate(
        &self,
        ns: impl Into<String>,
        ops: Vec<SettingsPathOp>,
        expected: Option<u64>,
    ) -> Result<(), BoxError> {
        self.write(ns.into(), WriteChange::Mutate(ops), expected)
            .await
    }

    async fn write(
        &self,
        ns: String,
        change: WriteChange,
        expected: Option<u64>,
    ) -> Result<(), BoxError> {
        let entry = self
            .0
            .editor
            .entries()
            .into_iter()
            .find(|row| row.options.id == ns)
            .ok_or_else(|| format!("No configurable plugin entry \"{ns}\""))?;
        let schema = self
            .0
            .editor
            .schema(&entry)?
            .ok_or_else(|| format!("No configurable plugin entry \"{ns}\""))?;
        let form = volatile_form(&schema)
            .ok_or_else(|| format!("Plugin entry \"{ns}\" has no volatile fields"))?;
        if let WriteChange::Mutate(ops) = &change {
            validate_mutation_paths(&schema, ops)?;
        }
        let service = self.clone();
        let transaction_ns = ns.clone();
        let transaction_schema = schema.clone();
        let transaction_form = form.clone();
        self.0
            .editor
            .try_edit(&entry, move |raw, inherited| async move {
                let descriptor = service
                    .describe(SettingsDescribeOptions::default())
                    .await?
                    .into_iter()
                    .find(|row| row.ns == transaction_ns.as_str())
                    .ok_or_else(|| {
                        format!(
                            "Plugin entry \"{}\" is no longer configurable",
                            transaction_ns
                        )
                    })?;
                if let Some(expected) = expected
                    && descriptor.revision != expected
                {
                    return Err(Box::new(SettingsConflictError {
                        namespace: transaction_ns.clone(),
                        expected,
                        actual: descriptor.revision,
                    }) as BoxError);
                }
                let current = object(project_form(&transaction_form, &raw))?;
                let base = object(project_form(&transaction_form, &inherited))?;
                let next = match change {
                    WriteChange::Update(patch) => merge_layers(Value::Object(current), patch),
                    WriteChange::Replace(section) => {
                        merge_layers(Value::Object(base.clone()), section)
                    }
                    WriteChange::Mutate(ops) => {
                        let mut value = Value::Object(current);
                        let base = Value::Object(base);
                        for op in &ops {
                            value = mutate_one(
                                value,
                                &base,
                                op,
                                &transaction_schema,
                                &transaction_schema,
                            )?;
                        }
                        value
                    }
                };
                require_object(&next)?;
                validate_paths(
                    &next,
                    &transaction_schema,
                    &transaction_form,
                    &transaction_form,
                    &mut vec![],
                )?;
                let ordinary = strip_volatile(
                    &raw,
                    &transaction_schema,
                    &transaction_form,
                    &transaction_form,
                    &mut vec![],
                );
                Ok(merge_layers(ordinary, next))
            })
            .await?;
        self.describe(SettingsDescribeOptions::default()).await?;
        Ok(())
    }

    async fn import_legacy_document(&self) -> Result<(), BoxError> {
        let path = self.0.editor.home().join("settings.yaml");
        if !tokio::fs::try_exists(&path).await? {
            return Ok(());
        }
        let imported = PathBuf::from(format!("{}.imported", path.to_string_lossy()));
        tokio::fs::rename(&path, &imported).await?;
        let text = tokio::fs::read_to_string(&imported).await?;
        let document = yaml::parse(&text).map_err(|error| -> BoxError { error.into() })?;
        let sections = match document {
            Value::Null => Map::new(),
            Value::Object(sections) => sections,
            _ => return Err("legacy settings document must be a mapping".into()),
        };
        for (section, values) in sections {
            let ns = legacy_namespace(&section);
            if let Err(error) = self.update(ns.clone(), values, None).await {
                self.0.ctx.logger().warn(format_args!(
                    "settings: section {section} of {} was not imported into entry {ns}",
                    imported.display()
                ));
                self.0.ctx.logger().warn(error);
            }
        }
        self.0.ctx.logger().info(format_args!(
            "settings: imported {} into profile {}",
            imported.display(),
            self.0.editor.profile_name()
        ));
        Ok(())
    }
}

impl SettingsApi for SettingsForms {
    fn configure(
        &self,
        presentation: SettingsPresentation,
        owner: Fiber,
    ) -> Result<SettingsUndo, BoxError> {
        SettingsForms::configure(self, presentation, owner)
    }
    fn writable(&self) -> bool {
        SettingsForms::writable(self)
    }
    fn document_path(&self) -> PathBuf {
        SettingsForms::document_path(self)
    }
    fn prepare_document(&self) -> SettingsFuture<PathBuf> {
        let service = self.clone();
        Box::pin(async move { SettingsForms::prepare_document(&service).await })
    }
    fn describe(
        &self,
        options: SettingsDescribeOptions,
    ) -> SettingsFuture<Vec<SettingsDescriptor>> {
        let service = self.clone();
        Box::pin(async move { SettingsForms::describe(&service, options).await })
    }
    fn update(&self, ns: String, patch: Value, expected: Option<u64>) -> SettingsFuture<()> {
        let service = self.clone();
        Box::pin(async move { SettingsForms::update(&service, ns, patch, expected).await })
    }
    fn replace(&self, ns: String, section: Value, expected: Option<u64>) -> SettingsFuture<()> {
        let service = self.clone();
        Box::pin(async move { SettingsForms::replace(&service, ns, section, expected).await })
    }
    fn mutate(
        &self,
        ns: String,
        ops: Vec<SettingsPathOp>,
        expected: Option<u64>,
    ) -> SettingsFuture<()> {
        let service = self.clone();
        Box::pin(async move { SettingsForms::mutate(&service, ns, ops, expected).await })
    }
}

#[derive(Inject)]
pub struct SettingsDeps {
    editor: Service<ConfigEditorKey>,
    loader: Service<LoaderKey>,
    _profile: Service<ProfileContextKey>,
}
pub struct SettingsPlugin;
impl Plugin for SettingsPlugin {
    const NAME: &'static str = "Settings";
    type Config = NoConfig;
    type Deps = SettingsDeps;
    async fn apply(ctx: Context, deps: SettingsDeps, _: Cfg<NoConfig>) -> Result<(), BoxError> {
        let (settings, mut refresh) = SettingsForms::create(ctx.clone(), deps.editor);
        let weak = Arc::downgrade(&settings);
        ctx.spawn("settings refresh", move |cancel| async move {
            loop {
                tokio::select! { _ = cancel.cancelled() => break, item = refresh.recv() => if item.is_none() { break }, }
                tokio::task::yield_now().await;
                let Some(settings) = weak.upgrade() else { break; };
                settings.0.scheduled.store(false, Ordering::Release);
                if settings.0.closed.load(Ordering::Acquire) || settings.0.ctx.fiber().state() != FiberState::Active { continue; }
                if let Err(error) = settings.describe(SettingsDescribeOptions::default()).await { settings.0.ctx.logger().error(error); }
            }
            Ok(())
        })?;
        let invalidator = Arc::downgrade(&settings);
        ctx.on::<ConfigReload>(move |_| {
            if let Some(settings) = invalidator.upgrade() {
                settings.invalidate();
            }
        })?;
        let closed = Arc::downgrade(&settings);
        ctx.try_effect("settings lifetime", move |undos| -> Result<(), BoxError> {
            undos.push(Undo::sync(move || {
                if let Some(settings) = closed.upgrade() {
                    settings.0.closed.store(true, Ordering::Release);
                }
            }))?;
            Ok(())
        })?;
        let service: Arc<dyn SettingsApi> = settings.clone();
        ctx.provide::<Settings>(service).await?;
        let loader = deps.loader;
        let legacy = Arc::downgrade(&settings);
        ctx.spawn("settings legacy import", move |_| async move {
            loader.settled().await;
            if let Some(settings) = legacy.upgrade() {
                settings.import_legacy_document().await?;
            }
            Ok(())
        })?;
        Ok(())
    }
}

fn require_object(value: &Value) -> Result<&Map<String, Value>, BoxError> {
    value
        .as_object()
        .ok_or_else(|| "Config $ contains a non-plain root".into())
}
fn object(value: Value) -> Result<Map<String, Value>, BoxError> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "Config root must be a plain object".into())
}

fn merge_layers(under: Value, over: Value) -> Value {
    match (under, over) {
        (Value::Object(mut under), Value::Object(over)) => {
            for (key, value) in over {
                let next = under
                    .remove(&key)
                    .map_or(value.clone(), |previous| merge_layers(previous, value));
                under.insert(key, next);
            }
            Value::Object(under)
        }
        (_, over) => over,
    }
}

fn lookup<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut value = value;
    for key in path {
        value = match value {
            Value::Object(object) => object.get(key)?,
            Value::Array(array) => array.get(strict_array_index(key)?)?,
            _ => return None,
        };
    }
    Some(value)
}

fn mutate_one(
    current: Value,
    base: &Value,
    op: &SettingsPathOp,
    root_schema: &Value,
    schema: &Value,
) -> Result<Value, BoxError> {
    fn edit(
        input: Option<&Value>,
        path: &[String],
        op: &SettingsPathOp,
        root_schema: &Value,
        schema: &Value,
    ) -> Result<Option<Value>, BoxError> {
        let Some((head, rest)) = path.split_first() else {
            return Ok(match op {
                SettingsPathOp::Set { value, .. } => Some(value.clone()),
                SettingsPathOp::Unset { .. } => None,
            });
        };
        let value = input.or_else(|| schema.get("default"));
        if let Some(Value::Array(array)) = value {
            if head != "0"
                && (head.starts_with('0') || !head.bytes().all(|byte| byte.is_ascii_digit()))
            {
                return Err(format!("Config array index \"{head}\" is out of range").into());
            }
            let index = head
                .parse::<usize>()
                .map_err(|_| format!("Config array index \"{head}\" is out of range"))?;
            let unset = rest.is_empty() && matches!(op, SettingsPathOp::Unset { .. });
            if index > array.len() || index == array.len() && (!rest.is_empty() || unset) {
                return Err(format!("Config array index \"{head}\" is out of range").into());
            }
            let mut result = array.clone();
            if unset {
                result.remove(index);
            } else {
                let child_schema =
                    schema::child_schema(root_schema, schema, head).unwrap_or(&Value::Null);
                let child = edit(array.get(index), rest, op, root_schema, child_schema)?
                    .unwrap_or(Value::Null);
                if index == result.len() {
                    result.push(child);
                } else {
                    result[index] = child;
                }
            }
            return Ok(Some(Value::Array(result)));
        }
        let mut result = value
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let child_schema = schema::child_schema(root_schema, schema, head).unwrap_or(&Value::Null);
        let child = edit(result.get(head), rest, op, root_schema, child_schema)?;
        match child {
            Some(child) => {
                result.insert(head.clone(), child);
            }
            None => {
                result.remove(head);
            }
        }
        Ok(Some(Value::Object(result)))
    }
    let effective = match op {
        SettingsPathOp::Unset { path } => {
            let parent = path
                .get(..path.len().saturating_sub(1))
                .and_then(|path| lookup(&current, path));
            if parent.is_some_and(Value::is_array) {
                op.clone()
            } else if let Some(inherited) = lookup(base, path) {
                SettingsPathOp::Set {
                    path: path.clone(),
                    value: inherited.clone(),
                }
            } else {
                op.clone()
            }
        }
        op => op.clone(),
    };
    let result = edit(
        Some(&current),
        effective.path(),
        &effective,
        root_schema,
        schema,
    )?
    .unwrap_or(Value::Null);
    object(result).map(Value::Object)
}

fn validate_paths(
    value: &Value,
    schema: &Value,
    form_root: &Value,
    form_node: &Value,
    path: &mut Vec<String>,
) -> Result<(), BoxError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    for (key, child) in object {
        path.push(key.clone());
        if !is_volatile_path(schema, path) {
            let field = schema::child_schema(form_root, form_node, key);
            if child.is_object()
                && let Some(field) = field
            {
                validate_paths(child, schema, form_root, field, path)?;
            } else {
                return Err(format!("Config field \"{}\" is not volatile", path.join(".")).into());
            }
        }
        path.pop();
    }
    Ok(())
}

fn validate_mutation_paths(schema: &Value, ops: &[SettingsPathOp]) -> Result<(), BoxError> {
    for op in ops {
        if !op.path().is_empty() && !is_volatile_path(schema, op.path()) {
            return Err(format!("Config field \"{}\" is not volatile", op.path().join(".")).into());
        }
    }
    Ok(())
}

fn strip_volatile(
    value: &Value,
    schema: &Value,
    form_root: &Value,
    form_node: &Value,
    path: &mut Vec<String>,
) -> Value {
    if is_volatile_path(schema, path) {
        return json!({});
    }
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    let mut result = object.clone();
    let Some(properties) = schema::object_properties(form_root, form_node) else {
        return Value::Object(result);
    };
    for (key, field) in properties {
        path.push(key.clone());
        if is_volatile_path(schema, path) {
            result.remove(key);
        } else if let Some(child) = result.get(key).cloned()
            && child.is_object()
        {
            result.insert(
                key.clone(),
                strip_volatile(&child, schema, form_root, field, path),
            );
        }
        path.pop();
    }
    Value::Object(result)
}

fn strict_array_index(key: &str) -> Option<usize> {
    if key == "0"
        || key
            .strip_prefix(|first: char| ('1'..='9').contains(&first))
            .is_some_and(|rest| rest.bytes().all(|byte| byte.is_ascii_digit()))
    {
        key.parse().ok()
    } else {
        None
    }
}

fn legacy_namespace(section: &str) -> String {
    match section {
        "ui-developer-tools" => "ui-settings".into(),
        "ui-onboarding" => "ui-settings-general".into(),
        "shell" if cfg!(windows) => "pwsh-sandbox".into(),
        "shell" => "bash-sandbox".into(),
        section => section.into(),
    }
}

#[cfg(test)]
#[path = "../tests/settings.rs"]
mod tests;
