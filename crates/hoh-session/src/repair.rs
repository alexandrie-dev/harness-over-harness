use crate::{Result, SessionEvent, SessionSeq};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::sync::Arc;
pub const TOOL_NOT_STARTED: &str = "TOOL_NOT_STARTED";
pub const TOOL_OUTCOME_UNKNOWN: &str = "TOOL_OUTCOME_UNKNOWN";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OpenTurnCloseCause {
    #[default]
    Interrupted,
    Forked,
}
impl OpenTurnCloseCause {
    pub fn kind(self) -> &'static str {
        match self {
            Self::Interrupted => "interrupted",
            Self::Forked => "forked",
        }
    }
}
#[derive(Clone)]
struct Pending {
    turn: Value,
    step: Value,
    call_seq: Option<Value>,
}
#[derive(Clone)]
struct LastObserved {
    seq: Option<Value>,
    time: Option<Value>,
}
// Recovery reads a possibly damaged tail before Session admission. Keep raw counters
// until the source's postfix increment admits a newly generated SessionSeq.
enum RecoverySequence {
    Number(f64),
    Text(String),
}
impl RecoverySequence {
    fn after(raw: Option<&Value>, additions: &[usize]) -> Self {
        let mut result = match raw {
            Some(Value::Number(number)) => Self::Number(number.as_f64().unwrap_or(f64::NAN)),
            Some(Value::Null) => Self::Number(0.0),
            Some(Value::Bool(value)) => Self::Number(if *value { 1.0 } else { 0.0 }),
            Some(value) => Self::Text(hoh_util_values::js_string(value)),
            None => Self::Number(f64::NAN),
        };
        for addition in additions {
            match &mut result {
                Self::Number(number) => *number += *addition as f64,
                Self::Text(text) => text.push_str(&addition.to_string()),
            }
        }
        result
    }
    fn text(&self) -> String {
        match self {
            Self::Number(number) => hoh_util_values::js_number_string(*number),
            Self::Text(text) => text.clone(),
        }
    }
    fn next(&mut self) -> Result<SessionSeq> {
        let number = match self {
            Self::Number(number) => *number,
            Self::Text(text) => {
                let text = hoh_util_values::js_trim(text);
                let radix = [("0x", "0X", 16), ("0o", "0O", 8), ("0b", "0B", 2)]
                    .into_iter()
                    .find_map(|(lower, upper, radix)| {
                        text.strip_prefix(lower)
                            .or_else(|| text.strip_prefix(upper))
                            .map(|s| (s, radix))
                    });
                if let Some((digits, radix)) = radix {
                    if digits.is_empty() {
                        f64::NAN
                    } else {
                        digits
                            .chars()
                            .try_fold(0.0, |n, ch| {
                                ch.to_digit(radix)
                                    .map(|digit| n * radix as f64 + digit as f64)
                            })
                            .unwrap_or(f64::NAN)
                    }
                } else if text.is_empty() {
                    0.0
                } else {
                    text.parse().unwrap_or(f64::NAN)
                }
            }
        };
        let seq = SessionSeq::from_number(number)?;
        *self = Self::Number(number + 1.0);
        Ok(seq)
    }
}
pub(crate) fn closer_event_value(
    kind: &str,
    seq: SessionSeq,
    time: Option<&Value>,
    data: Value,
) -> Value {
    let mut event = json!({"type":kind,"seq":seq});
    if let Some(time) = time {
        event["time"] = time.clone();
    }
    event["data"] = data;
    event
}
#[derive(Clone, Default)]
pub struct ToolCallRecovery {
    cause: OpenTurnCloseCause,
    pending_calls: IndexMap<hoh_llm::ToolCallId, Pending>,
    last: Option<LastObserved>,
}
impl ToolCallRecovery {
    pub fn new(cause: OpenTurnCloseCause) -> Self {
        Self {
            cause,
            ..Self::default()
        }
    }
    pub fn observe(&mut self, e: &SessionEvent) {
        self.last = Some(LastObserved {
            seq: e.as_value().get("seq").cloned(),
            time: e.as_value().get("time").cloned(),
        });
        let d = e.data();
        match e.event_type() {
            "turn/start" | "turn/end" | "step/end" => self.pending_calls.clear(),
            "assistant/message" => {
                for block in d["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool-call"
                        && let Some(id) = block["id"].as_str()
                    {
                        self.pending_calls.insert(
                            id.into(),
                            Pending {
                                turn: d["turn"].clone(),
                                step: d["step"].clone(),
                                call_seq: None,
                            },
                        );
                    }
                }
            }
            "tool/call" => {
                if let Some(id) = d["callId"].as_str()
                    && let Some(p) = self.pending_calls.get_mut(id)
                {
                    p.call_seq = e.as_value().get("seq").cloned();
                }
            }
            "tool/result" => {
                if let Some(id) = d["message"]["source"]["callId"].as_str()
                    && let Some(p) = self.pending_calls.get(id)
                    && e.surface_op() == Some(&json!("append"))
                    && p.turn == d["turn"]
                    && p.step == d["step"]
                {
                    self.pending_calls.shift_remove(id);
                }
            }
            _ => {}
        }
    }
    pub fn results(&self) -> Result<Vec<Arc<SessionEvent>>> {
        let Some(last) = &self.last else {
            return Ok(vec![]);
        };
        let mut results = vec![];
        let mut cursor = RecoverySequence::after(last.seq.as_ref(), &[1]);
        for (id, p) in &self.pending_calls {
            let started = p.call_seq.is_some();
            let text = match (self.cause, started) {
                (OpenTurnCloseCause::Interrupted, true) => {
                    "The tool call was interrupted after it was recorded, but no result was durably recorded. Its outcome is unknown. Decide whether to retry from the tool semantics: retry only if the operation is read-only or idempotent; if it may have side effects, first verify external state or ask the user. Do not retry blindly."
                }
                (OpenTurnCloseCause::Interrupted, false) => {
                    "The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed."
                }
                (OpenTurnCloseCause::Forked, true) => {
                    "The history inherited by this branch records this tool call starting but does not include its result. The parent session may have completed it after the fork point. Decide whether to retry from the tool semantics: retry only if the operation is read-only or idempotent; if it may have side effects, first verify external state or ask the user. Do not retry blindly."
                }
                (OpenTurnCloseCause::Forked, false) => {
                    "The history inherited by this branch has no record of this tool call starting. The parent session may have executed it after the fork point. Decide whether to retry from the tool semantics: retry only if the operation is read-only or idempotent; if it may have side effects, first verify external state or ask the user. Do not retry blindly."
                }
            };
            let id_seq = cursor.text();
            let seq = cursor.next()?;
            let mut event = closer_event_value(
                "tool/result",
                seq,
                last.time.as_ref(),
                json!({
                    "turn":p.turn,"step":p.step,"message":{
                        "id":format!("{}-tool-result-{id}-{id_seq}",self.cause.kind()),"role":"tool","toolCallId":id,"isError":true,
                        "source":{"kind":"tool","callId":id},"content":[{"type":"text","text":text}]
                    },"error":if started { json!({"name":"ToolOutcomeUnknownError","code":TOOL_OUTCOME_UNKNOWN}) } else { json!({"name":"ToolNotStartedError","code":TOOL_NOT_STARTED}) }
                }),
            );
            event["surfaceOp"] = json!("append");
            if let Some(call_seq) = &p.call_seq {
                event["sourceEventSeqs"] = json!([call_seq]);
            }
            results.push(Arc::new(SessionEvent::from_value(event)));
        }
        Ok(results)
    }
}
pub fn open_turn_closers(
    events: &[Arc<SessionEvent>],
    cause: OpenTurnCloseCause,
) -> Result<Vec<Arc<SessionEvent>>> {
    let mut turn = None;
    let mut step = None;
    let mut recovery = ToolCallRecovery::new(cause);
    for e in events {
        recovery.observe(e);
        match e.event_type() {
            "turn/start" => {
                turn = Some(e.data()["turn"].clone());
                step = None;
            }
            "turn/end" => {
                turn = None;
                step = None;
            }
            "step/start" => {
                step = Some(e.data()["step"].clone());
            }
            "step/end" => {
                step = None;
            }
            _ => {}
        }
    }
    let (Some(turn), Some(last)) = (turn, events.last()) else {
        return Ok(vec![]);
    };
    let mut closers = recovery.results()?;
    let mut cursor = RecoverySequence::after(last.as_value().get("seq"), &[closers.len(), 1]);
    let time = last.as_value().get("time");
    if let Some(step) = step {
        let seq = cursor.next()?;
        closers.push(Arc::new(SessionEvent::from_value(closer_event_value(
            "step/end",
            seq,
            time,
            json!({"turn":turn,"step":step}),
        ))));
    }
    let seq = cursor.next()?;
    closers.push(Arc::new(SessionEvent::from_value(closer_event_value(
        "turn/end",
        seq,
        time,
        json!({"turn":turn,"reason":{"kind":cause.kind()}}),
    ))));
    Ok(closers)
}
pub fn interrupted_turn_closers(events: &[Arc<SessionEvent>]) -> Result<Vec<Arc<SessionEvent>>> {
    open_turn_closers(events, OpenTurnCloseCause::Interrupted)
}
