//! First transport: caller-owned newline-delimited byte streams (including stdio).
//! Mirrors dsh transport.ts, including its lack of a maximum line size.
use crate::framing::{JsonRpcId, RpcError};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
};

#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    Request {
        id: JsonRpcId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: JsonRpcId,
        result: Value,
        error: Option<RpcError>,
    },
}
fn object_params(value: Option<&Value>) -> Value {
    value
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}))
}
fn ecma_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000a}'
            | '\u{000b}'
            | '\u{000c}'
            | '\u{000d}'
            | '\u{0020}'
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}
pub fn classify(value: Value) -> Option<Incoming> {
    let object = value.as_object()?;
    let id = object
        .get("id")
        .and_then(|v| serde_json::from_value::<JsonRpcId>(v.clone()).ok());
    let method = object.get("method").and_then(Value::as_str);
    match (id, method) {
        (Some(id), Some(method)) => Some(Incoming::Request {
            id,
            method: method.into(),
            params: object_params(object.get("params")),
        }),
        (Some(id), None) => {
            let error = object
                .get("error")
                .filter(|v| v.is_object() || v.is_array())
                .map(|e| RpcError {
                    code: e.get("code").and_then(Value::as_i64),
                    message: e
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("JSON-RPC error")
                        .into(),
                    data: e.get("data").cloned(),
                });
            Some(Incoming::Response {
                id,
                result: object.get("result").cloned().unwrap_or(Value::Null),
                error,
            })
        }
        (None, Some(method)) => Some(Incoming::Notification {
            method: method.into(),
            params: object_params(object.get("params")),
        }),
        _ => None,
    }
}
pub struct FrameReader<R> {
    input: R,
    line: Vec<u8>,
    max_line_bytes: Option<usize>,
}
impl<R: BufRead> FrameReader<R> {
    pub fn new(input: R) -> Self {
        Self {
            input,
            line: Vec::new(),
            max_line_bytes: None,
        }
    }
    /// Bounded buffering for network transports; None preserves dsh stdio behavior.
    pub fn with_line_limit(input: R, max_line_bytes: Option<usize>) -> Self {
        Self {
            input,
            line: Vec::new(),
            max_line_bytes,
        }
    }
    fn read_line(&mut self) -> io::Result<usize> {
        let Some(limit) = self.max_line_bytes else {
            return self.input.read_until(b'\n', &mut self.line);
        };
        let mut count = 0usize;
        let mut oversized = false;
        loop {
            let available = self.input.fill_buf()?;
            if available.is_empty() {
                break;
            }
            let newline = available.iter().position(|b| *b == b'\n');
            let take = newline.map_or(available.len(), |i| i + 1);
            let payload = take - usize::from(newline.is_some());
            if count.saturating_add(payload) > limit {
                oversized = true;
            }
            if !oversized {
                self.line.extend_from_slice(&available[..take]);
            }
            count = count.saturating_add(take);
            self.input.consume(take);
            if newline.is_some() {
                break;
            }
        }
        if oversized {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSON-RPC line exceeds configured limit",
            ));
        }
        Ok(count)
    }
    pub fn read(&mut self) -> io::Result<Option<Incoming>> {
        loop {
            self.line.clear();
            if self.read_line()? == 0 {
                return Ok(None);
            }
            // dsh drains only complete lines; an EOF tail is never a frame.
            if self.line.last() != Some(&b'\n') {
                return Ok(None);
            }
            let text = String::from_utf8_lossy(&self.line);
            if let Ok(value) = serde_json::from_str(text.trim_matches(ecma_whitespace))
                && let Some(frame) = classify(value)
            {
                return Ok(Some(frame));
            }
        }
    }
}
pub struct FrameWriter<W> {
    output: W,
}
impl<W: Write> FrameWriter<W> {
    pub fn new(output: W) -> Self {
        Self { output }
    }
    pub fn write<T: Serialize>(&mut self, message: &T) -> io::Result<()> {
        serde_json::to_writer(&mut self.output, message)?;
        self.output.write_all(b"\n")?;
        self.output.flush()
    }
    pub fn into_inner(self) -> W {
        self.output
    }
}
/// Pending correlation is channel-independent. Abandoned/late responses are dropped.
/// The embedding async host owns waiters; close returns all waiters to reject.
#[derive(Default)]
pub struct PendingRequests {
    pending: HashMap<String, String>,
    failure: Option<String>,
}
impl PendingRequests {
    pub fn register(&mut self, id: &JsonRpcId, method: &str) -> Result<(), String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if self.pending.contains_key(&id.key()) {
            return Err("duplicate pending id".into());
        }
        self.pending.insert(id.key(), method.into());
        Ok(())
    }
    pub fn abandon(&mut self, id: &JsonRpcId) {
        self.pending.remove(&id.key());
    }
    pub fn receive(&mut self, response: Incoming) -> Option<(String, Result<Value, RpcError>)> {
        if let Incoming::Response { id, result, error } = response {
            self.pending
                .remove(&id.key())
                .map(|method| (method, error.map_or(Ok(result), Err)))
        } else {
            None
        }
    }
    pub fn close(&mut self, reason: &str) -> Vec<String> {
        self.failure.get_or_insert_with(|| reason.into());
        self.pending.drain().map(|(_, method)| method).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn source_line_rules_and_fragmented_utf8() {
        let input =
            "\n  \r\n{bad}\nnull\n[]\n{\"method\":\"hi\",\"params\":[1]}\n{\"method\":\"tail\"}";
        let mut reader = FrameReader::new(io::BufReader::with_capacity(1, Cursor::new(input)));
        assert_eq!(
            reader.read().unwrap(),
            Some(Incoming::Notification {
                method: "hi".into(),
                params: json!({})
            })
        );
        assert_eq!(reader.read().unwrap(), None);
        let mut bytes = format!("{{\"method\":\"{}я\"}}\n", "x".repeat(2_000_000)).into_bytes();
        bytes.extend_from_slice(b"{\"id\":4,\"result\":true}\n");
        let mut reader = FrameReader::new(io::BufReader::with_capacity(1, Cursor::new(bytes)));
        assert!(
            matches!(reader.read().unwrap(), Some(Incoming::Notification { method, .. }) if method.ends_with('я'))
        );
        assert!(matches!(
            reader.read().unwrap(),
            Some(Incoming::Response { .. })
        ));
    }
    #[test]
    fn pending_errors_late_answers_and_permanent_close() {
        let id = JsonRpcId::String("a".into());
        let mut pending = PendingRequests::default();
        let response = classify(json!({"id":"a","error":{"data":{"why":1}}})).unwrap();
        assert!(pending.receive(response.clone()).is_none());
        pending.register(&id, "initialize").unwrap();
        let (_, Err(error)) = pending.receive(response.clone()).unwrap() else {
            panic!()
        };
        assert_eq!(error.message, "JSON-RPC error");
        assert_eq!(error.code, None);
        pending.register(&id, "initialize").unwrap();
        pending.abandon(&id);
        assert!(pending.receive(response).is_none());
        pending.register(&id, "initialize").unwrap();
        assert_eq!(pending.close("closed"), vec!["initialize"]);
        assert!(pending.register(&id, "initialize").is_err());
        assert!(pending.close("other").is_empty());
    }
    #[test]
    fn writer_and_numeric_ids() {
        let mut pending = PendingRequests::default();
        pending
            .register(&JsonRpcId::Number(1.into()), "initialize")
            .unwrap();
        assert!(
            pending
                .receive(classify(json!({"id":1.0,"result":{}})).unwrap())
                .is_some()
        );
        let mut writer = FrameWriter::new(Vec::new());
        writer
            .write(&json!({"jsonrpc":"2.0","id":1,"result":{}}))
            .unwrap();
        assert_eq!(
            writer.into_inner(),
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n"
        );
        assert!(
            matches!(classify(json!({"id":3.5,"method":"a","params":null})), Some(Incoming::Request { params, .. }) if params == json!({}))
        );
    }
}
