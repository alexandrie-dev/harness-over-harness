use serde::{Deserialize, Serialize, ser::SerializeMap};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "operation")]
pub enum DomainChanged {
    #[serde(rename = "put")]
    Put(DomainChangedPut),
    #[serde(rename = "deleted")]
    Deleted(DomainChangedDeleted),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainChangedBase {
    pub domain: String,
    pub table: String,
    pub key: String,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct DomainChangedPut {
    pub domain: String,
    pub table: String,
    pub key: String,
    pub value: Value,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct DomainChangedDeleted {
    pub domain: String,
    pub table: String,
    pub key: String,
}
impl Serialize for DomainChangedPut {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(5))?;
        map.serialize_entry("domain", &self.domain)?;
        map.serialize_entry("table", &self.table)?;
        map.serialize_entry("key", &self.key)?;
        map.serialize_entry("operation", "put")?;
        map.serialize_entry("value", &self.value)?;
        map.end()
    }
}
impl Serialize for DomainChangedDeleted {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("domain", &self.domain)?;
        map.serialize_entry("table", &self.table)?;
        map.serialize_entry("key", &self.key)?;
        map.serialize_entry("operation", "deleted")?;
        map.end()
    }
}
impl Serialize for DomainChanged {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Put(value) => value.serialize(serializer),
            Self::Deleted(value) => value.serialize(serializer),
        }
    }
}
hoh_kernel::event!(DomainChanged = "domain/changed", hoh_kernel::Emit);
