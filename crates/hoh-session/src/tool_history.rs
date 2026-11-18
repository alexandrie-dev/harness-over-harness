use crate::types::seq_number;
use crate::{Result, SessionError, SessionEvent, SessionSeq};
use hoh_util_values::js_json_stringify;
use indexmap::{IndexMap, IndexSet};
use serde_json::{Value, json};
use std::sync::Arc;
#[derive(Clone)]
pub struct ToolHistoryProjection {
    headers: IndexMap<SessionSeq, Vec<Value>>,
    declared: IndexMap<String, Value>,
    active: Vec<Value>,
    available: IndexSet<String>,
    baseline_seq: Option<SessionSeq>,
    history: Arc<Value>,
}
impl Default for ToolHistoryProjection {
    fn default() -> Self {
        Self {
            headers: Default::default(),
            declared: Default::default(),
            active: vec![],
            available: Default::default(),
            baseline_seq: None,
            history: Arc::new(json!({"tools":[],"updates":[]})),
        }
    }
}
impl ToolHistoryProjection {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn apply(&mut self, e: &SessionEvent) -> Result<()> {
        let d = e.data();
        if e.event_type() == "request/header" {
            let tools = d["header"]["tools"].as_array().cloned().unwrap_or_default();
            self.headers.insert(e.seq(), tools.clone());
            let redeclared = tools.iter().any(|t| {
                t["name"]
                    .as_str()
                    .and_then(|n| self.declared.get(n))
                    .is_some_and(|before| js_json_stringify(before) != js_json_stringify(t))
            });
            if self.baseline_seq.is_none()
                || d["reason"] == "series"
                || d["startsSeries"] == true
                || redeclared
            {
                self.baseline_seq = Some(e.seq());
                self.declared = tools
                    .iter()
                    .filter_map(|t| Some((t["name"].as_str()?.to_owned(), t.clone())))
                    .collect();
                self.available = self.declared.keys().cloned().collect();
                self.history = Arc::new(json!({"tools":tools,"updates":[]}));
            }
            self.active = tools;
        } else if e.event_type() == "developer/message" {
            let definitions = d
                .get("headerSeq")
                .and_then(seq_number)
                .and_then(|n| SessionSeq::new(n).ok())
                .and_then(|seq| self.headers.get(&seq));
            let mut additions = vec![];
            for block in d["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool-addition" {
                    let name = block["toolName"].as_str().unwrap_or("");
                    let tool = definitions
                        .and_then(|tools| tools.iter().find(|t| t["name"] == name))
                        .ok_or_else(|| {
                            SessionError::message(format!(
                                "tool history: missing definition for {name}"
                            ))
                        })?;
                    additions.push(tool.clone());
                }
            }
            for t in &additions {
                if let Some(name) = t["name"].as_str() {
                    self.declared.insert(name.into(), t.clone());
                }
            }
            for block in d["message"]["content"].as_array().into_iter().flatten() {
                if let Some(name) = block["toolName"].as_str() {
                    if block["type"] == "tool-addition" {
                        self.available.insert(name.into());
                    } else if block["type"] == "tool-removal" {
                        self.available.shift_remove(name);
                    }
                }
            }
            let mut history = (*self.history).clone();
            // The initial snapshot always contains an updates array.
            history["updates"]
                .as_array_mut()
                .expect("tool history updates array")
                .push(json!({"messageId":d["message"]["id"],"additions":additions}));
            self.history = Arc::new(history);
        }
        Ok(())
    }
    pub fn snapshot(&self) -> Arc<Value> {
        if self.active.len() != self.available.len()
            || self.active.iter().any(|t| {
                !t["name"]
                    .as_str()
                    .is_some_and(|n| self.available.contains(n))
            })
        {
            return Arc::new(json!({"tools":self.active,"updates":[]}));
        }
        self.history.clone()
    }
    pub fn llm_snapshot(&self) -> std::result::Result<hoh_llm::ToolHistory, serde_json::Error> {
        serde_json::from_value((*self.snapshot()).clone())
    }
}
