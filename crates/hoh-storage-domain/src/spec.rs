use crate::{
    domain::{DomainGlobal, DomainInner, KvTable},
    error::PlainError,
};
use hoh_error::BoxError;
use hoh_storage::{KvLayout, KvUnitDescriptor, UNIT_NAME_RE};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::Any, marker::PhantomData, sync::Arc};

pub(crate) type Data = Arc<dyn Any + Send + Sync>;
type Parser<T> = Arc<dyn Fn(&Value) -> Result<T, BoxError> + Send + Sync>;
type ErasedParser = Arc<dyn Fn(&Value) -> Result<Data, BoxError> + Send + Sync>;
type HandleFactory = Arc<dyn Fn(Arc<DomainInner>, String) -> Data + Send + Sync>;

/// The owner's stored-value decoder, including stripping/defaults/transforms.
/// It is deliberately separate from the kernel's plugin-config JSON Schema.
pub struct ValueSchema<T> {
    parser: Parser<T>,
}
impl<T> Clone for ValueSchema<T> {
    fn clone(&self) -> Self {
        Self {
            parser: self.parser.clone(),
        }
    }
}
impl<T> ValueSchema<T> {
    pub fn new(parser: impl Fn(&Value) -> Result<T, BoxError> + Send + Sync + 'static) -> Self {
        Self {
            parser: Arc::new(parser),
        }
    }
    pub fn parse(&self, value: &Value) -> Result<T, BoxError> {
        (self.parser)(value)
    }
}

pub struct DomainTableSpec<K = String, V = Value> {
    pub value_schema: ValueSchema<V>,
    key: PhantomData<fn(K)>,
}
pub fn domain_table<K, V>(schema: ValueSchema<V>) -> DomainTableSpec<K, V> {
    DomainTableSpec {
        value_schema: schema,
        key: PhantomData,
    }
}
pub trait TableDefinition {
    type Key;
    type Value;
}
impl<K, V> TableDefinition for DomainTableSpec<K, V> {
    type Key = K;
    type Value = V;
}
pub type TableKeyOf<T> = <T as TableDefinition>::Key;
pub type TableValueOf<T> = <T as TableDefinition>::Value;

#[derive(Clone)]
pub struct ErasedTableSpec {
    pub(crate) parser: ErasedParser,
    pub(crate) handle: HandleFactory,
}
impl<K, V> From<DomainTableSpec<K, V>> for ErasedTableSpec
where
    K: AsRef<str> + From<String> + Send + Sync + 'static,
    V: Serialize + Send + Sync + 'static,
{
    fn from(spec: DomainTableSpec<K, V>) -> Self {
        Self {
            parser: Arc::new(move |value| Ok(Arc::new(spec.value_schema.parse(value)?) as Data)),
            handle: Arc::new(|inner, name| Arc::new(KvTable::<K, V>::new(inner, name)) as Data),
        }
    }
}
pub struct DomainGlobalSpec<G = Value> {
    pub schema: ValueSchema<G>,
    pub initial: G,
}
pub trait GlobalDefinition {
    type Value;
}
impl<G> GlobalDefinition for DomainGlobalSpec<G> {
    type Value = G;
}
pub type GlobalValueOf<T> = <T as GlobalDefinition>::Value;
pub type DomainGlobalHandleOf<T> = DomainGlobal<GlobalValueOf<T>>;
#[derive(Clone)]
pub struct ErasedGlobalSpec {
    pub(crate) parser: ErasedParser,
    pub(crate) initial: Data,
    pub(crate) handle: HandleFactory,
}
impl<G: Serialize + Send + Sync + 'static> From<DomainGlobalSpec<G>> for ErasedGlobalSpec {
    fn from(spec: DomainGlobalSpec<G>) -> Self {
        Self {
            parser: Arc::new(move |value| Ok(Arc::new(spec.schema.parse(value)?) as Data)),
            initial: Arc::new(spec.initial),
            handle: Arc::new(|inner, _| Arc::new(DomainGlobal::<G>::new(inner)) as Data),
        }
    }
}

/// Unchecked declaration boundary: invalid numeric/layout/policy inputs remain
/// representable so define_domain reports the source's diagnostics.
#[derive(Clone)]
pub struct DomainSpecInput {
    pub name: String,
    pub version: Value,
    pub tables: IndexMap<String, ErasedTableSpec>,
    pub layout: Option<String>,
    pub compatible_versions: Option<Vec<Value>>,
    pub invalid_records: Option<String>,
    pub global: Option<ErasedGlobalSpec>,
}
impl DomainSpecInput {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<Value>,
        tables: IndexMap<String, ErasedTableSpec>,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            tables,
            layout: None,
            compatible_versions: None,
            invalid_records: None,
            global: None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvalidRecords {
    #[serde(rename = "backup-and-skip")]
    BackupAndSkip,
}
#[derive(Clone)]
pub struct DomainSpec {
    pub name: String,
    pub version: u64,
    pub tables: IndexMap<String, ErasedTableSpec>,
    pub layout: Option<KvLayout>,
    pub compatible_versions: Option<Vec<u64>>,
    pub invalid_records: Option<InvalidRecords>,
    pub global: Option<ErasedGlobalSpec>,
}
fn natural(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|n| {
                n.is_finite() && *n >= 0.0 && n.fract() == 0.0 && *n <= 9_007_199_254_740_992.0
            })
            .map(|n| n as u64)
    })
}
pub fn define_domain(spec: DomainSpecInput) -> Result<DomainSpec, BoxError> {
    let invalid = |message| -> BoxError { Box::new(PlainError(message)) };
    if !UNIT_NAME_RE.is_match(&spec.name) {
        return Err(invalid(format!(
            "domain name '{}' must match /^[a-z][a-z0-9_]*$/",
            spec.name
        )));
    }
    let version = natural(&spec.version).ok_or_else(|| {
        invalid(format!(
            "domain '{}' version must be a non-negative integer, got {}",
            spec.name,
            hoh_util_values::js_string(&spec.version)
        ))
    })?;
    let mut compatible = spec.compatible_versions.as_ref().map(|_| Vec::new());
    for value in spec.compatible_versions.as_deref().unwrap_or(&[]) {
        let parsed = natural(value).filter(|v| *v < version).ok_or_else(|| invalid(format!("domain '{}' compatibleVersions entries must be non-negative integers below version {}, got {}",spec.name,version,hoh_util_values::js_string(value))))?;
        if let Some(versions) = &mut compatible {
            versions.push(parsed);
        }
    }
    let layout = match spec.layout.as_deref() {
        None => None,
        Some("single") => Some(KvLayout::Single),
        Some("per-record") => Some(KvLayout::PerRecord),
        Some(value) => {
            return Err(invalid(format!(
                "domain '{}' layout must be 'single' or 'per-record', got {value}",
                spec.name
            )));
        }
    };
    let policy = match spec.invalid_records.as_deref() {
        None => None,
        Some("backup-and-skip") => Some(InvalidRecords::BackupAndSkip),
        Some(value) => {
            return Err(invalid(format!(
                "domain '{}' invalidRecords must be 'backup-and-skip' when present, got {value}",
                spec.name
            )));
        }
    };
    for table in spec.tables.keys() {
        if !UNIT_NAME_RE.is_match(table) {
            return Err(invalid(format!(
                "domain '{}' table name '{table}' must match /^[a-z][a-z0-9_]*$/",
                spec.name
            )));
        }
    }
    if spec
        .global
        .as_ref()
        .is_some_and(|global| (global.parser)(&Value::Null).is_ok())
    {
        return Err(invalid(format!(
            "domain '{}' global schema must not accept null: null is the medium's \"never written\" sentinel, so a stored null could not round-trip",
            spec.name
        )));
    }
    Ok(DomainSpec {
        name: spec.name,
        version,
        tables: spec.tables,
        layout,
        compatible_versions: compatible,
        invalid_records: policy,
        global: spec.global,
    })
}
pub fn descriptor_of(spec: &DomainSpec) -> KvUnitDescriptor {
    KvUnitDescriptor {
        name: spec.name.clone(),
        version: spec.version,
        tables: spec.tables.keys().cloned().collect(),
        has_global: spec.global.is_some(),
        layout: spec.layout,
        compatible_versions: spec.compatible_versions.clone(),
    }
}
