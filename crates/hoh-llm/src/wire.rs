//! Ordered JSON snapshots for the objects retained in provider streams.
use serde_json::{Map, Value};
use std::sync::Arc;

/// Original wire fields and the typed baseline. Use Default for newly built values.
#[derive(Clone, Debug, Default)]
pub struct JsonSnapshot {
    snapshot: Option<Arc<SnapshotFields>>,
}
#[derive(Debug)]
struct SnapshotFields {
    original: Map<String, Value>,
    baseline: Map<String, Value>,
}
impl PartialEq for JsonSnapshot {
    fn eq(&self, other: &Self) -> bool {
        let extras = |s: &Self| {
            let Some(s) = &s.snapshot else {
                return Map::new();
            };
            s.original
                .iter()
                .filter(|(k, _)| !s.baseline.contains_key(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Map<_, _>>()
        };
        extras(self) == extras(other)
    }
}
impl JsonSnapshot {
    pub(crate) fn capture(original: Value, baseline: Value) -> Self {
        Self {
            snapshot: Some(Arc::new(SnapshotFields {
                original: original.as_object().cloned().unwrap_or_default(),
                baseline: baseline.as_object().cloned().unwrap_or_default(),
            })),
        }
    }
    pub(crate) fn merge(&self, current: Value) -> Value {
        let Some(snapshot) = &self.snapshot else {
            return current;
        };
        let Some(current) = current.as_object() else {
            return current;
        };
        let mut result = Map::new();
        for (key, original) in &snapshot.original {
            match current.get(key) {
                Some(value) => {
                    result.insert(
                        key.clone(),
                        if snapshot.baseline.get(key) == Some(value) {
                            original.clone()
                        } else {
                            value.clone()
                        },
                    );
                }
                None if !snapshot.baseline.contains_key(key) => {
                    result.insert(key.clone(), original.clone());
                }
                None => {}
            }
        }
        for (key, value) in current {
            if !result.contains_key(key) {
                result.insert(key.clone(), value.clone());
            }
        }
        Value::Object(result)
    }
}

macro_rules! lossless_struct {
    ($name:ident, [$($derive:path),*], [$($attr:meta),*], { $($(#[$field_attr:meta])* pub $field:ident: $ty:ty,)* }) => {
        #[derive($($derive),*)]
        pub struct $name {
            $(pub $field: $ty,)*
            /// Ordered, otherwise unknown fields captured at the JSON boundary.
            pub wire: $crate::wire::JsonSnapshot,
        }
        const _: () = {
            #[derive(serde::Serialize, serde::Deserialize)]
            $(#[$attr])*
            struct Fields {
                $($(#[$field_attr])* $field: $ty,)*
            }
            impl serde::Serialize for $name {
                fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok,S::Error> {
                    let fields = Fields { $($field: self.$field.clone(),)* };
                    let value = serde_json::to_value(fields).map_err(serde::ser::Error::custom)?;
                    self.wire.merge(value).serialize(serializer)
                }
            }
            impl<'de> serde::Deserialize<'de> for $name {
                fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self,D::Error> {
                    let original = serde_json::Value::deserialize(deserializer)?;
                    let fields: Fields = serde_json::from_value(original.clone()).map_err(serde::de::Error::custom)?;
                    let baseline = serde_json::to_value(&fields).map_err(serde::de::Error::custom)?;
                    Ok(Self { $($field: fields.$field,)* wire: $crate::wire::JsonSnapshot::capture(original, baseline) })
                }
            }
        };
    };
}
pub(crate) use lossless_struct;
