#![allow(dead_code)]
pub mod resource;
use hoh_kernel::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
pub use std::time::Duration;
pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}
pub async fn turn() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}
pub struct Track {
    pub log: Mutex<Vec<String>>,
    pub contexts: Mutex<Vec<Context>>,
    pub configs: Mutex<Vec<Cfg<Value>>>,
    pub updates: Mutex<Vec<Value>>,
    pub gate: tokio::sync::Semaphore,
    pub cleanup: tokio::sync::Semaphore,
}
impl Default for Track {
    fn default() -> Self {
        Self {
            log: Mutex::default(),
            contexts: Mutex::default(),
            configs: Mutex::default(),
            updates: Mutex::default(),
            gate: tokio::sync::Semaphore::new(0),
            cleanup: tokio::sync::Semaphore::new(0),
        }
    }
}
impl Track {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            gate: tokio::sync::Semaphore::new(0),
            cleanup: tokio::sync::Semaphore::new(0),
            ..Self::default()
        })
    }
    pub fn push(&self, s: impl Into<String>) {
        lock(&self.log).push(s.into());
    }
    pub fn list(&self) -> Vec<String> {
        lock(&self.log).clone()
    }
    pub fn context(&self) -> Context {
        lock(&self.contexts).last().unwrap().clone()
    }
}
#[plugin(Recorded, name = "recorded")]
pub async fn recorded(ctx: Context, config: Cfg<Value>) -> Result<(), Error> {
    let t = ctx.tag::<Arc<Track>>().unwrap();
    let t = (*t).clone();
    let c = config.get();
    let n = c["value"].as_i64().unwrap_or(0);
    t.push(format!("load {n}"));
    lock(&t.contexts).push(ctx.clone());
    lock(&t.configs).push(config);
    if c["hook"] == true {
        let t = t.clone();
        ctx.on_update::<Value, _>(move |c, next| {
            let t = t.clone();
            async move {
                lock(&t.updates).push((*c).clone());
                if c["reject"] == true {
                    return Err("rejected update".into());
                }
                if c["delegate"] == true {
                    next.run().await?;
                }
                Ok(())
            }
        })?;
    }
    let t2 = t.clone();
    let cleanup = c["cleanup"] == true;
    let cleanup_error = c["cleanupError"] == true;
    ctx.effect_async("recorded", move |u| async move {
        if c["wait"] == true {
            t2.gate.acquire().await.unwrap().forget();
        }
        if let Some(ms) = c["delay"].as_u64() {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        let t3 = t2.clone();
        u.push(Undo::future(move || async move {
            t3.push(format!("unload {n}"));
            if cleanup {
                t3.cleanup.acquire().await.unwrap().forget();
            }
            if cleanup_error {
                return Err("cleanup failure".into());
            }
            Ok(())
        }))?;
        if c["fail"] == true {
            return Err::<(), Error>(format!("failure {n}").into());
        }
        Ok(())
    })
    .await?;
    Ok(())
}
pub fn harness() -> (Kernel, Context, Arc<Track>) {
    let kernel = Kernel::start();
    let t = Track::new();
    let root = kernel.root().with_tag(t.clone());
    (kernel, root, t)
}
pub struct A;
impl ServiceKey for A {
    type Value = i32;
    const NAME: &'static str = "foo";
    fn view(v: &i32) -> Option<Value> {
        Some(json!({"value":v}))
    }
}
service!(pub B:i32="bar");
#[derive(Inject)]
pub struct ADeps {
    pub a: Service<A>,
}
#[derive(Inject)]
pub struct ABDeps {
    pub a: Service<A>,
    pub b: Service<B>,
}
pub struct Missing;
impl ServiceKey for Missing {
    type Value = i32;
    const NAME: &'static str = "never-provided";
}
#[derive(Inject)]
pub struct MissingDeps {
    pub missing: Service<Missing>,
}
pub struct Ping;
event!(Ping = "custom-event", Emit);
pub struct Par;
event!(Par = "parallel", Parallel);
pub struct Ser;
event!(Ser = "serial", Serial<i32>);
pub struct Stop;
event!(Stop = "bail", Bail<i32>);
pub struct Fall;
event!(Fall = "waterfall", Waterfall<i32>);
pub struct ResolveOk;
impl ConfigResolver for ResolveOk {
    fn resolve(&self, _: &Context, raw: &Value) -> Result<Value, Error> {
        if raw["ok"] != true {
            return Err("not ok".into());
        }
        let mut value = raw.clone();
        value["resolved"] = json!(true);
        Ok(value)
    }
}
