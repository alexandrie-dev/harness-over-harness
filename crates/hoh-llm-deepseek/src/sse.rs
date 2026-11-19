//! Incremental Server-Sent Events framing and JSON validation.

use crate::transport::provider_error;
use hoh_error::HarnessError;
use hoh_llm::{BuiltinLlmFailureCode as Code, LlmError};
use serde_json::Value;

pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SseOverflow;

#[derive(Default)]
pub struct SseParser {
    line: Vec<u8>,
    skip_lf: bool,
    started: bool,
    data: Option<String>,
    event: Option<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, SseOverflow> {
        let mut events = Vec::new();
        for &byte in bytes {
            if std::mem::take(&mut self.skip_lf) && byte == b'\n' {
                continue;
            }
            match byte {
                b'\n' | b'\r' => {
                    self.skip_lf = byte == b'\r';
                    let line = std::mem::take(&mut self.line);
                    self.process_line(&line, &mut events);
                }
                _ => {
                    if self.line.len() >= MAX_LINE_BYTES {
                        return Err(SseOverflow);
                    }
                    self.line.push(byte);
                }
            }
        }
        Ok(events)
    }
    pub fn finish(&mut self) -> Option<SseEvent> {
        None
    }
    fn process_line(&mut self, raw: &[u8], events: &mut Vec<SseEvent>) {
        let mut line = String::from_utf8_lossy(raw).into_owned();
        if !self.started {
            self.started = true;
            if let Some(rest) = line.strip_prefix('\u{feff}') {
                line = rest.into();
            }
        }
        if line.is_empty() {
            let event = self.event.take();
            if let Some(mut data) = self.data.take() {
                data.pop();
                events.push(SseEvent { event, data });
            }
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = line
            .split_once(':')
            .map_or((line.as_str(), ""), |(field, value)| {
                (field, value.strip_prefix(' ').unwrap_or(value))
            });
        match field {
            "data" => {
                let data = self.data.get_or_insert_with(String::new);
                data.push_str(value);
                data.push('\n');
            }
            "event" => self.event = Some(value.into()),
            _ => {}
        }
    }
}

fn malformed(message: &str) -> HarnessError {
    LlmError::new(message, Code::MalformedResponse).expect("SSE diagnostic is valid")
}

pub fn decode_event(frame: SseEvent) -> Result<Value, HarnessError> {
    let value: Value = serde_json::from_str(&frame.data)
        .map_err(|_| malformed("DeepSeek Messages SSE contains invalid JSON"))?;
    let object = value
        .as_object()
        .ok_or_else(|| malformed("DeepSeek Messages expected a JSON object"))?;
    let event_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed("DeepSeek Messages SSE event type mismatch"))?;
    if frame
        .event
        .as_deref()
        .is_some_and(|name| name != event_type)
    {
        return Err(malformed("DeepSeek Messages SSE event type mismatch"));
    }
    if event_type == "error" {
        return Err(provider_error(&value, None, None));
    }
    Ok(value)
}
