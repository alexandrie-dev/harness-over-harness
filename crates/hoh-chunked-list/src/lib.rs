//! Persistent append-only lists with bounded copying and strict JSON checkpoints.
use serde::{
    Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned, ser::SerializeStruct,
};
use serde_json::Value;
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
impl<T> Drop for ChunkedList<T> {
    fn drop(&mut self) {
        let mut previous = self.previous.take();
        while let Some(node) = previous {
            match Arc::into_inner(node) {
                Some(mut owned) => previous = owned.previous.take(),
                None => break,
            }
        }
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

pub fn iterate_chunked_list<T>(head: Option<&Arc<ChunkedList<T>>>) -> impl Iterator<Item = &T> {
    let mut chunks = Vec::new();
    let mut current = head.map(Arc::as_ref);
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

impl<'de, T: DeserializeOwned> Deserialize<'de> for ChunkedList<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Parsing JSON belongs to the deserializer; restoring the parsed chain is iterative.
        let head = chunked_list_schema(Value::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)?;
        Arc::try_unwrap(head)
            .map_err(|_| serde::de::Error::custom("unexpected shared restored head"))
    }
}

// Error paths must also dispose of an unvisited, arbitrarily deep JSON suffix without recursion.
struct JsonGuard(Value);
impl Drop for JsonGuard {
    fn drop(&mut self) {
        let mut pending = vec![self.0.take()];
        while let Some(value) = pending.pop() {
            match value {
                Value::Array(values) => pending.extend(values),
                Value::Object(object) => pending.extend(object.into_values()),
                _ => {}
            }
        }
    }
}
fn invalid(message: &str) -> serde_json::Error {
    serde::de::Error::custom(message)
}

/// Restore a programmatically constructed checkpoint without recursive traversal of `previous`.
/// The element's `Deserialize` implementation replaces the caller's Zod schema.
pub fn chunked_list_schema<T: DeserializeOwned>(
    value: Value,
) -> Result<Arc<ChunkedList<T>>, serde_json::Error> {
    let mut current = JsonGuard(value);
    let mut chunks = Vec::new();
    loop {
        let object = current
            .0
            .as_object_mut()
            .ok_or_else(|| invalid("chunk must be an object"))?;
        if object
            .keys()
            .any(|key| key != "values" && key != "previous")
        {
            return Err(invalid("unknown chunk field"));
        }
        let values = object
            .get("values")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("values must be an array"))?;
        if values.is_empty() || values.len() > CHUNK_CAPACITY {
            return Err(invalid("chunk must contain 1 to 64 values"));
        }
        let values = object
            .remove("values")
            .ok_or_else(|| invalid("missing values"))?;
        let values: Vec<T> = serde_json::from_value(values)?;
        chunks.push(values.into_iter().map(Arc::new).collect());
        match object.remove("previous") {
            Some(previous) => current.0 = previous,
            None => break,
        }
    }
    let mut head = None;
    for values in chunks.into_iter().rev() {
        head = Some(Arc::new(ChunkedList {
            values,
            previous: head,
        }));
    }
    head.ok_or_else(|| invalid("missing chunk"))
}
