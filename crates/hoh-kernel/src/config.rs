use crate::{Value as Erased, read};
use serde_json::Value;
use std::{
    fmt,
    sync::{Arc, RwLock},
};

pub trait Config:
    serde::de::DeserializeOwned + serde::Serialize + schemars::JsonSchema + Send + Sync + 'static
{
}
impl<T> Config for T where
    T: serde::de::DeserializeOwned
        + serde::Serialize
        + schemars::JsonSchema
        + Send
        + Sync
        + 'static
{
}
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct NoConfig {}
pub struct Cfg<C: Config> {
    pub(crate) value: Arc<RwLock<Erased>>,
    pub(crate) marker: std::marker::PhantomData<C>,
}
impl<C: Config> Clone for Cfg<C> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            marker: std::marker::PhantomData,
        }
    }
}
impl<C: Config> Cfg<C> {
    pub fn get(&self) -> Arc<C> {
        // PluginDef creates and updates this cell only with its associated Config type.
        read(&self.value)
            .clone()
            .downcast::<C>()
            .expect("typed configuration cell")
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Volatile<T>(pub T);
impl<T> std::ops::Deref for Volatile<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: schemars::JsonSchema> schemars::JsonSchema for Volatile<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("Volatile_{}", T::schema_name()).into()
    }
    fn inline_schema() -> bool {
        true
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut schema = T::json_schema(generator);
        if schema.get("x-volatile") == Some(&Value::Bool(true)) {
            schema.insert(
                "x-volatile-invalid".into(),
                Value::String("already wrapped".into()),
            );
        }
        schema.insert("x-volatile".into(), Value::Bool(true));
        schema
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum PathSeg {
    Key(String),
    Index(usize),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub message: String,
    pub path: Vec<PathSeg>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ValidationError {
    pub issues: Vec<Issue>,
}
impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid config:")?;
        for issue in &self.issues {
            write!(f, "\n  - {}", issue.message)?;
            if !issue.path.is_empty() {
                let path = issue
                    .path
                    .iter()
                    .map(|s| match s {
                        PathSeg::Key(k) => k.clone(),
                        PathSeg::Index(i) => i.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(".");
                write!(f, " (at {path})")?;
            }
        }
        Ok(())
    }
}
impl std::error::Error for ValidationError {}
pub(crate) fn issue(message: impl Into<String>) -> ValidationError {
    ValidationError {
        issues: vec![Issue {
            message: message.into(),
            path: vec![],
        }],
    }
}

mod diff;
mod schema;
pub use diff::{equal_except_volatile, volatile_paths};
pub use schema::validate;
pub(crate) use schema::{coerce_integers, merge, normalized};
