#![allow(dead_code)]
use super::*;
use hoh_kernel::{
    Cfg, Context, Error, FiberState, Kernel, Plugin, Service, Undo, Volatile, VolatileUpdate,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Mutex;

pub fn locked<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap()
}
#[derive(Default)]
pub struct Observed {
    pub values: Mutex<Vec<Value>>,
    pub events: Mutex<Vec<(u32, Vec<String>, Value)>>,
    pub disposed: Mutex<Vec<u32>>,
    pub configs: Mutex<Vec<Cfg<LiveConfig>>>,
    pub hooks: Mutex<Vec<Value>>,
}
pub struct Noop;
impl Plugin for Noop {
    const NAME: &'static str = "noop";
    type Config = Value;
    type Deps = ();
    async fn apply(ctx: Context, _: (), config: Cfg<Value>) -> Result<(), Error> {
        if let Some(state) = ctx.tag::<Observed>() {
            locked(&state.values).push((*config.get()).clone());
            let id = ctx.fiber().uid();
            ctx.effect("test cleanup", move || {
                Undo::sync(move || locked(&state.disposed).push(id))
            })?;
        }
        Ok(())
    }
}
pub struct Fails;
impl Plugin for Fails {
    const NAME: &'static str = "fails";
    type Config = Value;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<Value>) -> Result<(), Error> {
        Err("activation boom".into())
    }
}
pub struct Objects;
impl Plugin for Objects {
    const NAME: &'static str = "objects";
    type Config = LiveConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), config: Cfg<LiveConfig>) -> Result<(), Error> {
        let state = ctx.tag::<Observed>().ok_or("missing test state")?;
        locked(&state.configs).push(config.clone());
        let id = ctx.fiber().uid();
        let value = config.clone();
        let events = state.clone();
        ctx.on::<VolatileUpdate>(move |event| {
            locked(&events.events).push((
                id,
                event.paths.clone(),
                serde_json::to_value(&*value.get()).unwrap(),
            ))
        })?;
        ctx.effect("test cleanup", move || {
            Undo::sync(move || locked(&state.disposed).push(id))
        })?;
        Ok(())
    }
}
pub struct Custom;
impl Plugin for Custom {
    const NAME: &'static str = "custom";
    type Config = LiveConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), config: Cfg<LiveConfig>) -> Result<(), Error> {
        Objects::apply(ctx.clone(), (), config).await?;
        let state = ctx.tag::<Observed>().unwrap();
        ctx.on_update::<LiveConfig, _>(move |next, _| {
            let state = state.clone();
            async move {
                locked(&state.hooks).push(serde_json::to_value(&*next).unwrap());
                Ok(())
            }
        })?;
        Ok(())
    }
}
fn hello() -> Volatile<String> {
    Volatile("hello".into())
}
fn fixed() -> String {
    "fixed".into()
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LiveConfig {
    #[serde(default = "hello")]
    pub title: Volatile<String>,
    #[serde(default = "fixed")]
    pub fixed: String,
    #[serde(default)]
    #[schemars(schema_with = "inline_nested_schema")]
    // Source declares an inline object with default({}).
    pub nested: Nested,
    #[serde(default)]
    pub optional: Volatile<Option<String>>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Nested {
    #[serde(default)]
    #[schemars(range(min = 0))]
    pub count: Volatile<i64>,
}
fn inline_nested_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    <Nested as schemars::JsonSchema>::json_schema(generator)
}
pub struct World {
    pub kernel: Kernel,
    pub ctx: Context,
    pub loader: Service<LoaderKey>,
    pub dir: tempfile::TempDir,
    pub observed: Arc<Observed>,
}
impl World {
    pub async fn new(text: &str) -> Self {
        Self::registry(text, |_: &mut PluginRegistry| {}).await
    }
    pub async fn registry(text: &str, configure: impl FnOnce(&mut PluginRegistry)) -> Self {
        Self::registry_env(text, configure, env).await
    }
    pub async fn registry_env(
        text: &str,
        configure: impl FnOnce(&mut PluginRegistry),
        make_env: impl FnOnce(&Path) -> EvalEnv,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cordis.yml");
        std::fs::write(&base, text).unwrap();
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let mut registry = PluginRegistry::default();
        registry
            .add::<Noop>("noop")
            .add::<Objects>("objects")
            .add::<Custom>("custom")
            .add::<Fails>("fails");
        configure(&mut registry);
        let ctx = ctx.with_tag(Observed::default());
        let actual = ctx.tag::<Observed>().unwrap();
        let loader = boot(
            &ctx,
            BootOptions {
                registry: Arc::new(registry),
                base,
                layers: vec![],
                env: make_env(dir.path()),
            },
        )
        .await
        .unwrap();
        Self {
            kernel,
            ctx,
            loader,
            dir,
            observed: actual,
        }
    }
    pub fn row(&self, id: &str) -> EntryInfo {
        self.loader
            .entries()
            .into_iter()
            .find(|e| e.options.id == id)
            .unwrap()
    }
    pub async fn patch(&self, text: &str) -> Result<Vec<EntryChange>, LoadError> {
        self.loader.recompose(vec![layer("edit", text)]).await
    }
    pub fn active(&self, id: &str) -> bool {
        self.row(id)
            .fiber
            .is_some_and(|f| f.state() == FiberState::Active)
    }
    pub async fn close(&self) {
        self.kernel.shutdown().await;
    }
}
pub fn single(config: Value) -> Vec<Entry> {
    serde_json::from_value(json!([{ "id":"x", "name":"objects", "config":config }])).unwrap()
}
