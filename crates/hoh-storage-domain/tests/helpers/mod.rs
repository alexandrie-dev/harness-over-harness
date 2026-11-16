pub mod memory_backend;
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_storage::{Storage, StoragePlugin};
use hoh_storage_domain::*;
use indexmap::IndexMap;
pub use memory_backend::{MemoryMediaPool, MemoryStorageBackend};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn received(value: Option<&Value>) -> &'static str {
    match value {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::String(_)) => "string",
        Some(Value::Number(_)) => "number",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}
fn invalid(expected: &str, value: Option<&Value>, path: Vec<Value>) -> Value {
    json!({"expected":expected,"code":"invalid_type","path":path,"message":format!("Invalid input: expected {expected}, received {}",received(value))})
}
pub fn item_schema() -> ValueSchema<Value> {
    ValueSchema::new(|value| {
        let Some(object) = value.as_object() else {
            return Err(Box::new(ZodError {
                issues: vec![invalid("object", Some(value), vec![])],
            }) as BoxError);
        };
        let mut issues = Vec::new();
        if !object.get("label").is_some_and(Value::is_string) {
            issues.push(invalid("string", object.get("label"), vec![json!("label")]));
        }
        match object.get("count") {
            Some(Value::Number(n)) => {
                let number = n.as_f64().unwrap();
                if number.fract() != 0.0 {
                    issues.push(json!({"expected":"int","format":"safeint","code":"invalid_type","path":["count"],"message":"Invalid input: expected int, received number"}));
                } else if number.abs() > 9_007_199_254_740_991.0 {
                    issues.push(if number>0.0 {
                        json!({"origin":"int","code":"too_big","maximum":9_007_199_254_740_991_i64,"inclusive":true,"path":["count"],"message":"Too big: expected int to be <=9007199254740991"})
                    } else {
                        json!({"origin":"int","code":"too_small","minimum":-9_007_199_254_740_991_i64,"inclusive":true,"path":["count"],"message":"Too small: expected int to be >=-9007199254740991"})
                    });
                }
            }
            value => issues.push(invalid("number", value, vec![json!("count")])),
        }
        if !issues.is_empty() {
            return Err(Box::new(ZodError { issues }) as BoxError);
        }
        Ok(json!({"label":object["label"],"count":object["count"]}))
    })
}
pub fn settings_schema() -> ValueSchema<Value> {
    ValueSchema::new(|value| {
        let Some(object) = value.as_object() else {
            return Err(Box::new(ZodError {
                issues: vec![invalid("object", Some(value), vec![])],
            }) as BoxError);
        };
        if !object.get("theme").is_some_and(Value::is_string) {
            return Err(Box::new(ZodError {
                issues: vec![invalid("string", object.get("theme"), vec![json!("theme")])],
            }) as BoxError);
        }
        Ok(json!({"theme":object["theme"]}))
    })
}
pub fn spec_input() -> DomainSpecInput {
    let mut input = DomainSpecInput::new(
        "demo",
        1,
        IndexMap::from([(
            "items".into(),
            domain_table::<String, Value>(item_schema()).into(),
        )]),
    );
    input.global = Some(
        DomainGlobalSpec {
            schema: settings_schema(),
            initial: json!({"theme":"plain"}),
        }
        .into(),
    );
    input
}
pub fn spec() -> Arc<DomainSpec> {
    Arc::new(define_domain(spec_input()).unwrap())
}
pub fn bare_spec() -> Arc<DomainSpec> {
    Arc::new(
        define_domain(DomainSpecInput::new(
            "bare",
            1,
            IndexMap::from([(
                "rows".into(),
                domain_table::<String, Value>(item_schema()).into(),
            )]),
        ))
        .unwrap(),
    )
}
pub fn table(domain: &Domain) -> Arc<KvTable> {
    domain.table::<String, Value>("items").unwrap()
}
pub fn global(domain: &Domain) -> Arc<DomainGlobal> {
    domain.global::<Value>().unwrap()
}
pub fn item(label: &str, count: i64) -> Value {
    json!({"label":label,"count":count})
}
pub struct Harness {
    pub kernel: Kernel,
    pub ctx: Context,
    pub backend: Arc<MemoryStorageBackend>,
    pub facility: DomainFacility,
    pub changes: Arc<Mutex<Vec<DomainChanged>>>,
    unmount: Box<dyn Fn() + Send + Sync>,
}
impl Harness {
    pub async fn new(pool: Arc<MemoryMediaPool>, config: Option<Config>) -> Self {
        Self::backend(MemoryStorageBackend::new(pool), config).await
    }
    pub async fn backend(backend: Arc<MemoryStorageBackend>, config: Option<Config>) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<StoragePlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let storage = ctx.get::<Storage>().unwrap();
        let _unregister = storage.backend.register("memory", backend.clone()).unwrap();
        let facility = DomainFacility::new(
            ctx.clone(),
            config.unwrap_or(Config {
                backend: "memory".into(),
                routes: IndexMap::new(),
            }),
        );
        let unmount = Box::new(storage.mount("domain", Arc::new(facility.clone())).unwrap());
        let changes = Arc::new(Mutex::new(Vec::new()));
        let seen = changes.clone();
        ctx.on::<DomainChanged>(move |event| seen.lock().unwrap().push(event.clone()))
            .unwrap();
        Self {
            kernel,
            ctx,
            backend,
            facility,
            changes,
            unmount,
        }
    }
    pub async fn stop(self) {
        self.facility.close_all().await.unwrap();
        (self.unmount)();
        self.kernel.shutdown().await;
    }
}
pub async fn harness() -> Harness {
    Harness::new(Arc::new(MemoryMediaPool::default()), None).await
}
pub fn code(error: &hoh_error::SharedError) -> &str {
    if let Some(e) = error.downcast_ref::<DomainError>() {
        e.code()
    } else if let Some(e) = error.downcast_ref::<hoh_storage::StorageError>() {
        e.code()
    } else {
        "plain"
    }
}
pub async fn settle_service(kernel: &Kernel, ctx: &Context, present: bool) {
    for _ in 0..1000 {
        let _ = kernel.snapshot().await;
        if ctx.get::<StorageDomain>().is_some() == present {
            return;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(ctx.get::<StorageDomain>().is_some(), present);
}
