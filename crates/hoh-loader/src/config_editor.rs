//! Explicit profile edits. Inclusion journals and implicit Fiber write-back are absent (O4).
use crate::{
    BundleSource, Entry, EntryInfo, EvalEnv, Layer, LayerSource, LoadError, LoaderKey, Profile,
    compose, parse_patches,
};
use futures_util::FutureExt;
use hoh_error::BoxError;
use hoh_kernel::{Context, FiberState, Service};
use indexmap::IndexMap;
use serde_json::Value;
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
};

hoh_kernel::service!(pub ConfigEditorKey: ConfigEditor = "configEditor");
#[derive(Clone)]
pub struct ConfigEditor {
    ctx: Context,
    dir: PathBuf,
    name: String,
    bundles: Arc<dyn BundleSource>,
    home: PathBuf,
    overlays: Vec<Layer>,
    env: EvalEnv,
    writes: Arc<tokio::sync::Mutex<()>>,
}
pub struct Configuration {
    pub entry: EntryInfo,
    pub inherited: Value,
    pub r#override: Value,
}

fn config_or_empty(config: Value) -> Value {
    if config.is_null() {
        serde_json::json!({})
    } else {
        config
    }
}

impl ConfigEditor {
    pub async fn mount(
        ctx: &Context,
        dir: PathBuf,
        bundles: Arc<dyn BundleSource>,
        home: PathBuf,
        overlays: Vec<PathBuf>,
        env: EvalEnv,
    ) -> Result<Service<ConfigEditorKey>, LoadError> {
        let mut frozen = Vec::new();
        for overlay in overlays {
            let file = crate::resolve_config_path(
                &overlay.to_string_lossy(),
                None,
                &env.cwd.to_string_lossy(),
            );
            let text = tokio::fs::read_to_string(&file)
                .await
                .map_err(|source| LoadError::Io {
                    file: file.clone(),
                    source,
                })?;
            frozen.push(Layer {
                source: LayerSource::CommandLine(file.clone()),
                file: Some(file.clone()),
                patches: parse_patches(&text, &file)?,
            });
        }
        Self::mount_with_overlays(ctx, dir, bundles, home, frozen, env).await
    }
    /// Launchers pass the invocation's already parsed, immutable overlay snapshot.
    pub async fn mount_with_overlays(
        ctx: &Context,
        dir: PathBuf,
        bundles: Arc<dyn BundleSource>,
        home: PathBuf,
        overlays: Vec<Layer>,
        env: EvalEnv,
    ) -> Result<Service<ConfigEditorKey>, LoadError> {
        let active = ctx.get::<crate::ProfileContextKey>();
        let editor = Arc::new(Self {
            ctx: ctx.clone(),
            name: active.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| {
                dir.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            }),
            dir,
            bundles,
            home: active.as_ref().map(|p| p.home.clone()).unwrap_or(home),
            overlays,
            env,
            writes: Arc::new(tokio::sync::Mutex::new(())),
        });
        ctx.provide::<ConfigEditorKey>(editor).await?;
        ctx.get::<ConfigEditorKey>()
            .ok_or_else(|| LoadError::Runtime("Configuration editor is no longer available".into()))
    }
    pub fn document_path(&self) -> PathBuf {
        self.dir.join("cordis.patch.yml")
    }
    /// Active profile home supplied by the host; never inferred from HOME.
    pub fn home(&self) -> &Path {
        &self.home
    }
    /// Active profile name, as loaded by Profile::load.
    pub fn profile_name(&self) -> &str {
        &self.name
    }
    pub fn schema(&self, entry: &EntryInfo) -> Result<Option<Value>, LoadError> {
        self.ctx
            .get::<LoaderKey>()
            .ok_or_else(|| LoadError::Runtime("Configuration entry is no longer available".into()))?
            .entry_schema(entry)
    }
    pub async fn resolve_inherited(
        &self,
        entry: &EntryInfo,
        raw: &Value,
    ) -> Result<Value, LoadError> {
        self.ctx
            .get::<LoaderKey>()
            .ok_or_else(|| LoadError::Runtime("Configuration entry is no longer available".into()))?
            .resolve_inherited(entry, raw)
            .await
    }
    pub fn entries(&self) -> Vec<EntryInfo> {
        let Some(loader) = self.ctx.get::<LoaderKey>() else {
            return vec![];
        };
        let candidates: Vec<_> = loader.profile_entries().into_iter().collect();
        let mut counts = IndexMap::<String, usize>::new();
        for entry in &candidates {
            *counts.entry(entry.options.id.clone()).or_default() += 1;
        }
        candidates
            .into_iter()
            .filter(|entry| counts.get(&entry.options.id) == Some(&1))
            .collect()
    }
    async fn profile(&self) -> Result<Profile, LoadError> {
        let (dir, source, home, overlays, env) = (
            self.dir.clone(),
            self.bundles.clone(),
            self.home.clone(),
            self.overlays.clone(),
            self.env.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let mut profile = Profile::load(&dir, source.as_ref(), &home, &[], &env)?;
            profile
                .layers
                .retain(|l| !matches!(l.source, LayerSource::Launcher(_)));
            profile.layers.extend(overlays);
            crate::profile::append_telemetry_patch(&mut profile.layers, &env);
            Ok(profile)
        })
        .await
        .map_err(|e| LoadError::Runtime(e.to_string()))?
    }
    fn inherited(entry: &EntryInfo, profile: &Profile) -> Value {
        let mut layers = profile.layers.clone();
        // Home and argv layers constrain effective values, but are not the inherited profile layer.
        layers
            .retain(|layer| matches!(layer.source, LayerSource::Bundle(_) | LayerSource::Profile));
        for layer in &mut layers {
            if matches!(layer.source, LayerSource::Profile) {
                for patch in &mut layer.patches {
                    if patch.id.as_ref().and_then(Value::as_str) == Some(&entry.options.id)
                        && patch.insert.is_none()
                    {
                        patch.set.shift_remove("config");
                    }
                }
            }
        }
        let composed = compose(&[], &layers, &mut |_| {});
        find(&composed.entries, &entry.options.id)
            .map(|e| config_or_empty(e.config))
            .unwrap_or_else(|| serde_json::json!({}))
    }
    pub async fn configuration(&self) -> Result<Vec<Configuration>, LoadError> {
        let profile = self.profile().await?;
        Ok(self
            .entries()
            .into_iter()
            .map(|entry| {
                let inherited = Self::inherited(&entry, &profile);
                let override_value = profile
                    .layers
                    .iter()
                    .filter(|l| matches!(l.source, LayerSource::Profile))
                    .flat_map(|l| &l.patches)
                    .rev()
                    .find(|p| {
                        p.id.as_ref().and_then(Value::as_str) == Some(&entry.options.id)
                            && p.set.contains_key("config")
                    })
                    .and_then(|p| p.set.get("config"))
                    .cloned()
                    .map(config_or_empty)
                    .unwrap_or_else(|| serde_json::json!({}));
                Configuration {
                    entry,
                    inherited,
                    r#override: override_value,
                }
            })
            .collect())
    }
    pub async fn edit(
        &self,
        entry: &EntryInfo,
        change: impl FnOnce(Value, Value) -> Value + Send + 'static,
    ) -> Result<(), LoadError> {
        self.try_edit(entry, |current, inherited| async move {
            Ok(change(current, inherited))
        })
        .await
        .map_err(|error| match error.downcast::<LoadError>() {
            Ok(error) => *error,
            Err(error) => match error.downcast::<hoh_kernel::KernelError>() {
                Ok(error) => LoadError::Kernel(*error),
                Err(error) => LoadError::AtomicWrite(error),
            },
        })
    }
    /// Reject an edit after reload and before validation/write, preserving the caller's error.
    pub async fn try_edit<F, Fut>(&self, entry: &EntryInfo, change: F) -> Result<(), BoxError>
    where
        F: FnOnce(Value, Value) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Value, BoxError>> + Send,
    {
        let editor = self.clone();
        let entry = entry.clone();
        let (reply, result) = tokio::sync::oneshot::channel();
        let task = self.ctx.spawn("configuration edit", move |_| async move {
            let result = editor.edit_serial(&entry, change).await;
            let _ = reply.send(result);
            Ok(())
        })?;
        let result = result.await.map_err(|_| hoh_kernel::KernelError::Closed);
        task.dispose().await;
        result?
    }
    async fn edit_serial<F, Fut>(&self, entry: &EntryInfo, change: F) -> Result<(), BoxError>
    where
        F: FnOnce(Value, Value) -> Fut + Send,
        Fut: Future<Output = Result<Value, BoxError>> + Send,
    {
        let _serial = self.writes.lock().await;
        hoh_atomic_write::with_file_lock(
            self.dir.join("package.json"),
            || async {
                let result = std::panic::AssertUnwindSafe(self.edit_locked(entry, change))
                    .catch_unwind()
                    .await;
                result.unwrap_or_else(|payload| {
                    Err(Box::new(LoadError::Runtime(
                        payload
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| {
                                payload
                                    .downcast_ref::<&str>()
                                    .map(|message| (*message).into())
                            })
                            .unwrap_or_else(|| "Configuration edit callback panicked".into()),
                    )) as BoxError)
                })
            },
            hoh_atomic_write::FileLockOptions::default(),
        )
        .await
    }
    async fn edit_locked<F, Fut>(&self, entry: &EntryInfo, change: F) -> Result<(), BoxError>
    where
        F: FnOnce(Value, Value) -> Fut + Send,
        Fut: Future<Output = Result<Value, BoxError>> + Send,
    {
        let loader = self.ctx.get::<LoaderKey>().ok_or_else(|| {
            LoadError::Runtime("Configuration entry is no longer available".into())
        })?;
        let available = |entries: Vec<EntryInfo>| {
            entries.into_iter().find(|e| {
                e.id == entry.id
                    && e.fiber.as_ref().map(|f| f.uid()) == entry.fiber.as_ref().map(|f| f.uid())
            })
        };
        if available(self.entries()).and_then(|e| e.fiber).is_none() {
            return Err(
                LoadError::Runtime("Configuration entry is no longer available".into()).into(),
            );
        }
        let profile = self.profile().await?;
        let previous_layers = profile.layers.clone();
        loader.recompose(previous_layers.clone()).await?;
        let current = available(self.entries()).ok_or_else(|| {
            LoadError::Runtime("Configuration entry changed during reload".into())
        })?;
        let inherited = Self::inherited(&current, &profile);
        let next = change(
            config_or_empty(current.options.config.clone()),
            inherited.clone(),
        )
        .await?;
        let fiber = current.fiber.ok_or_else(|| {
            LoadError::Runtime("Configuration entry is no longer available".into())
        })?;
        if fiber.state() != FiberState::Active {
            return Err(
                LoadError::Runtime("Configuration plugin is no longer active".into()).into(),
            );
        }
        fiber.resolve_config(&next)?;
        let path = self.document_path();
        let before = match tokio::fs::read_to_string(&path).await {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "[]\n".into(),
            Err(source) => return Err(LoadError::Io { file: path, source }.into()),
        };
        let text = crate::yaml_edit::edit_config(
            &before,
            &path,
            &entry.options.id,
            &entry.name,
            &next,
            hoh_util_values::deep_equal_json(&next, &inherited),
        )?;
        let patches = parse_patches(&text, &path)?;
        let mut layers = profile.layers;
        if let Some(layer) = layers
            .iter_mut()
            .find(|l| matches!(l.source, LayerSource::Profile))
        {
            layer.patches = patches.clone();
        } else {
            let position = layers
                .iter()
                .take_while(|l| matches!(l.source, LayerSource::Bundle(_)))
                .count();
            layers.insert(
                position,
                Layer {
                    source: LayerSource::Profile,
                    file: Some(path.clone()),
                    patches: patches.clone(),
                },
            );
        }
        let effective = compose(&[], &layers, &mut |_| {});
        if !hoh_util_values::deep_equal_json(
            &find(&effective.entries, &entry.options.id)
                .map(|e| config_or_empty(e.config))
                .unwrap_or_else(|| serde_json::json!({})),
            &next,
        ) {
            return Err(LoadError::Runtime(format!(
                "Configuration for \"{}\" is overridden by a home patch or command-line overlay",
                entry.options.id
            ))
            .into());
        }
        self.write_atomic(&text).await?;
        let result = loader.recompose(layers).await.and_then(|_| {
            if self.entries().iter().any(|current| {
                current.id == entry.id
                    && current
                        .fiber
                        .as_ref()
                        .is_some_and(|fiber| fiber.state() == FiberState::Active)
            }) {
                Ok(())
            } else {
                Err(LoadError::Runtime(
                    loader
                        .failure(&entry.id)
                        .unwrap_or_else(|| "Configuration plugin is no longer active".into()),
                ))
            }
        });
        if let Err(error) = result {
            self.write_atomic(&before).await?;
            loader.recompose(previous_layers).await?;
            return Err(error.into());
        }
        Ok(())
    }
    async fn write_atomic(&self, text: &str) -> Result<(), LoadError> {
        hoh_atomic_write::write_file_atomic(
            self.document_path(),
            text,
            hoh_atomic_write::WriteFileAtomicOptions {
                mode: 0o600,
                dir_mode: None,
            },
        )
        .await
        .map_err(|source| LoadError::Io {
            file: self.document_path(),
            source,
        })
    }
}
fn find(rows: &[Entry], id: &str) -> Option<Entry> {
    for row in rows {
        if row.id == id {
            return Some(row.clone());
        }
        if row.group
            && let Ok(children) = serde_json::from_value::<Vec<Entry>>(row.config.clone())
            && let Some(entry) = find(&children, id)
        {
            return Some(entry);
        }
    }
    None
}
