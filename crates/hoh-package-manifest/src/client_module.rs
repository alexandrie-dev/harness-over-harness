//! Executable browser declarations. Functions have no JSON representation.
use indexmap::IndexMap;
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};

pub trait ClientModuleStyleContext: Send + Sync {
    fn asset(&self, reference: &str) -> String;
}
pub struct ClientModuleStylesheetDescriptor {
    pub text: Arc<StylesheetText>,
}
pub type StylesheetText = dyn Fn(&dyn ClientModuleStyleContext) -> String + Send + Sync;
#[derive(Default)]
pub struct ClientModuleMeta {
    pub stylesheets: Option<IndexMap<String, ClientModuleStylesheetDescriptor>>,
}
pub type ClientModuleMetaFactory = Arc<dyn Fn() -> ClientModuleMeta + Send + Sync>;
pub trait ClientBundleRequire: Send + Sync {
    fn require(&self, specifier: &str) -> Value;
    fn r#async<'a>(
        &'a self,
        specifier: &'a str,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>>;
    fn asset(&self, reference: &str) -> String;
}
pub type ClientModuleFactory =
    Arc<dyn Fn(&dyn ClientBundleRequire) -> IndexMap<String, Value> + Send + Sync>;
pub struct DshClientModuleManifest {
    pub id: String,
    pub chunk: Option<String>,
    pub factory: ClientModuleFactory,
    pub meta: Option<ClientModuleMetaFactory>,
}
