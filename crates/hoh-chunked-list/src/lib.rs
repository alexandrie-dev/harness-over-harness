//! Persistent append-only lists with bounded copying and strict JSON checkpoints.
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeStruct};
use std::sync::Arc;
const CHUNK_CAPACITY: usize = 64;

#[derive(Debug)]
pub struct ChunkedList<T> {
    values: Vec<Arc<T>>,
    previous: Option<Arc<ChunkedList<T>>>,
}
impl<T> ChunkedList<T> {
    pub fn values(&self) -> &[Arc<T>] {
        &self.values
    }
    pub fn previous(&self) -> Option<&Arc<Self>> {
        self.previous.as_ref()
    }
}

pub fn append_chunked_list<T>(head: Option<&Arc<ChunkedList<T>>>, value: T) -> Arc<ChunkedList<T>> {
    let (mut values, previous) = match head {
        None => (Vec::new(), None),
        Some(head) if head.values.len() == CHUNK_CAPACITY => (Vec::new(), Some(head.clone())),
        Some(head) => (head.values.clone(), head.previous.clone()),
    };
    values.push(Arc::new(value));
    Arc::new(ChunkedList { values, previous })
}

pub fn iterate_chunked_list<T>(head: Option<&ChunkedList<T>>) -> impl Iterator<Item = &T> {
    let mut chunks = Vec::new();
    let mut current = head;
    while let Some(chunk) = current {
        chunks.push(chunk);
        current = chunk.previous.as_deref();
    }
    chunks
        .into_iter()
        .rev()
        .flat_map(|chunk| chunk.values.iter().map(Arc::as_ref))
}

impl<T: Serialize> Serialize for ChunkedList<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer
            .serialize_struct("ChunkedList", if self.previous.is_some() { 2 } else { 1 })?;
        state.serialize_field(
            "values",
            &self.values.iter().map(Arc::as_ref).collect::<Vec<_>>(),
        )?;
        if let Some(previous) = &self.previous {
            state.serialize_field("previous", previous.as_ref())?;
        }
        state.end()
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ChunkedList<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        fn previous<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Box<ChunkedList<T>>>, D::Error> {
            ChunkedList::deserialize(d).map(|v| Some(Box::new(v)))
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, bound(deserialize = "T: Deserialize<'de>"))]
        struct Raw<T> {
            values: Vec<T>,
            #[serde(default, deserialize_with = "previous")]
            previous: Option<Box<ChunkedList<T>>>,
        }
        let raw = Raw::<T>::deserialize(deserializer)?;
        if raw.values.is_empty() || raw.values.len() > CHUNK_CAPACITY {
            return Err(serde::de::Error::custom(
                "chunk must contain 1 to 64 values",
            ));
        }
        Ok(Self {
            values: raw.values.into_iter().map(Arc::new).collect(),
            previous: raw.previous.map(|p| Arc::new(*p)),
        })
    }
}

/// The element's `Deserialize` implementation replaces the caller's Zod schema.
pub fn chunked_list_schema<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
) -> Result<Arc<ChunkedList<T>>, serde_json::Error> {
    serde_json::from_value(value).map(Arc::new)
}
