//! Durable admission state for the loop-owned system prompt (surface node 0 and in-history
//! updates) and runtime context. The prompt registry supplies named contributions; this module owns
//! runtime reminder framing and supersession.

use hoh_error::BoxError;
use hoh_kernel::{Context, KernelError};
use hoh_llm::{
    ContentBlock, ContextSnapshotSection, MessageSource, NewUserMessage, SystemMessage,
    UserMessage, create_system_message, create_user_message,
};
use hoh_session::{
    Session, SessionEvent, SessionEventEmitted, SessionSeq, SurfaceIntent,
    is_replacement_surface_event,
};
use regex::Regex;
use serde_json::Value;
use std::sync::{Arc, Mutex, OnceLock};

const SOURCE: &str = "runtime-context";
const CLEARED: &str = "<system-reminder>\nCurrent runtime context: none. Earlier runtime-context snapshots no longer apply.\n</system-reminder>";

fn is_owned(message: &Value) -> bool {
    message.pointer("/source/kind").and_then(Value::as_str) == Some(SOURCE)
}

/// Older runtime-context messages may omit the structured snapshot contributions.
fn sections_of(message: &Value) -> Option<Vec<ContextSnapshotSection>> {
    let source = message.get("source")?;
    if source.get("kind")?.as_str()? != SOURCE || source.get("form")?.as_str()? != "snapshot" {
        return None;
    }
    serde_json::from_value(source.get("sections")?.clone()).ok()
}

/// One uncommitted system-prompt surface operation for request admission or reconciliation.
#[derive(Clone, Debug)]
pub struct SystemPromptCommit {
    /// Rendered prompt or empty content: an empty head records no prompt; empty tails are dormant.
    pub message: SystemMessage,
    /// `append` for a new system node, otherwise a replacement of one surviving system node.
    pub intent: SurfaceIntent,
}

/// The request-series facts one prompt decision is made under.
#[derive(Clone, Copy, Debug)]
pub struct SystemPromptDecisionInput {
    /// Whether the prepared route for this attempt reads a later `system` message as the prompt.
    pub in_history: bool,
    /// Whether this step starts a new request series.
    pub starts_series: bool,
}

/// Decides how a rendered system prompt reaches the surface without owning the commit. The first
/// prompt, even empty, reserves surface node 0.
pub struct SystemPromptProjection {
    session: Session,
}

struct SystemNode {
    seq: SessionSeq,
    text: Option<String>,
}

impl SystemPromptProjection {
    pub fn new(session: Session) -> Self {
        Self { session }
    }

    /// The surviving `system/message` nodes in surface order.
    fn system_nodes(&self) -> Result<Vec<SystemNode>, BoxError> {
        let mut nodes = Vec::new();
        for seq in self.session.surface()?.nodes()? {
            let Some(event) = self.session.event_at(seq) else {
                continue;
            };
            if event.event_type() != "system/message" {
                continue;
            }
            let message = event.data().get("message").unwrap_or(&Value::Null);
            let content = message.get("content").and_then(Value::as_array);
            let text = match content {
                Some(content) if content.is_empty() => Some(String::new()),
                // `textOf`: exactly one text block, else undefined.
                Some(content) if content.len() == 1 => {
                    let block = &content[0];
                    if block.get("type").and_then(Value::as_str) == Some("text") {
                        block.get("text").and_then(Value::as_str).map(str::to_owned)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            nodes.push(SystemNode { seq, text });
        }
        Ok(nodes)
    }

    /// Reconcile effective text and retained nodes with the prepared route and series. A series
    /// start re-baselines node 0 only when the effective text differs; a capable continuing series
    /// keeps appended nodes in place to preserve the prefix. An empty list means no update is needed.
    pub fn project(
        &self,
        rendered: &str,
        input: SystemPromptDecisionInput,
    ) -> Result<Vec<SystemPromptCommit>, BoxError> {
        let nodes = self.system_nodes()?;
        let Some(head) = nodes.first() else {
            return Ok(vec![SystemPromptCommit {
                message: create_system_message(rendered),
                intent: SurfaceIntent::append(),
            }]);
        };
        let latest = nodes
            .iter()
            .rev()
            .find(|node| node.text.as_deref() != Some(""))
            .unwrap_or(head);
        let latest_is_rendered = latest.text.as_deref() == Some(rendered);
        if !input.in_history || rendered.is_empty() || (input.starts_series && !latest_is_rendered)
        {
            let mut updates: Vec<SystemPromptCommit> = nodes[1..]
                .iter()
                .filter(|node| node.text.as_deref() != Some(""))
                .map(|node| Self::replace(node.seq, ""))
                .collect();
            if head.text.as_deref() != Some(rendered) {
                updates.push(Self::replace(head.seq, rendered));
            }
            return Ok(updates);
        }
        if latest_is_rendered {
            return Ok(vec![]);
        }
        Ok(vec![SystemPromptCommit {
            message: create_system_message(rendered),
            intent: SurfaceIntent::append(),
        }])
    }

    fn replace(seq: SessionSeq, text: &str) -> SystemPromptCommit {
        SystemPromptCommit {
            message: create_system_message(text),
            intent: SurfaceIntent::replace(seq, seq, vec![seq]),
        }
    }
}

#[derive(Clone)]
enum Retained {
    /// No snapshot ever existed (`undefined`).
    Never,
    /// None is retained (`null`).
    Cleared,
    Snapshot {
        seq: SessionSeq,
        sections: Option<Vec<ContextSnapshotSection>>,
    },
}

/// Tracks the last retained runtime-context snapshot without owning its commit.
#[derive(Clone)]
pub struct RuntimeContextProjection {
    retained: Arc<Mutex<Retained>>,
}

impl RuntimeContextProjection {
    /// Restore projection state once, then follow authoritative session events. `ctx` is the
    /// agent-scoped event context; the listener is its effect.
    pub fn new(ctx: &Context, session: &Session) -> Result<Self, BoxError> {
        let mut retained = Retained::Never;
        let surface: Vec<SessionSeq> = session.surface()?.nodes()?;
        for event in session.snapshot_events(None, None).iter().rev() {
            if event.event_type() != "user/message" || !is_owned(event.data()) {
                continue;
            }
            if matches!(retained, Retained::Never) {
                retained = Retained::Cleared;
            }
            if surface.contains(&event.seq()) {
                retained = Retained::Snapshot {
                    seq: event.seq(),
                    sections: sections_of(event.data()),
                };
                break;
            }
        }
        let retained = Arc::new(Mutex::new(retained));
        let (state, subject) = (retained.clone(), session.clone());
        ctx.on_contained::<SessionEventEmitted>(move |emitted| {
            if emitted.session != subject {
                return Ok(());
            }
            let event: &SessionEvent = &emitted.event;
            let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
            if event.event_type() == "user/message" && is_owned(event.data()) {
                *state = Retained::Snapshot {
                    seq: event.seq(),
                    sections: sections_of(event.data()),
                };
            } else if let Retained::Snapshot { seq, .. } = &*state
                && is_replacement_surface_event(event)
                && event
                    .source_event_seqs()
                    .and_then(Value::as_array)
                    .is_some_and(|seqs| seqs.iter().any(|s| s.as_u64() == Some(seq.get())))
            {
                *state = Retained::Cleared;
            }
            Ok(())
        })
        .map_err(|e: KernelError| Box::new(e) as BoxError)?;
        Ok(Self { retained })
    }

    /// Create an uncommitted reminder only when the retained value differs. The first snapshot
    /// omits supersession; later snapshots also override facts retained in compaction summaries.
    /// Proposals do not advance committed state.
    pub fn project(&self, sections: &[ContextSnapshotSection]) -> Option<UserMessage> {
        let retained = self
            .retained
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let previous = match &retained {
            Retained::Snapshot { sections, .. } => sections.as_deref(),
            _ => None,
        };
        let never = matches!(retained, Retained::Never);
        if never && sections.is_empty() {
            return None;
        }
        if previous.is_some_and(|previous| previous == sections) {
            return None;
        }
        let intro = if never {
            "The following information describes your current session:"
        } else {
            "The following information describes your current session. It replaces earlier versions of this context, including any retained in conversation summaries."
        };
        let body = sections
            .iter()
            .map(|section| section.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        static CLOSING: OnceLock<Regex> = OnceLock::new();
        let closing = CLOSING.get_or_init(|| {
            // JavaScript `\s` is not the `regex` crate's `\s`.
            // The JavaScript `i` flag (no `u`) folds ASCII letters only: spell the cases out.
            let name: String = "system-reminder"
                .chars()
                .map(|c| {
                    if c.is_ascii_alphabetic() {
                        format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
                    } else {
                        c.to_string()
                    }
                })
                .collect();
            Regex::new(&format!(
                "</{name}[{}]*>",
                hoh_util_values::JS_WHITE_SPACE_SET
            ))
            .expect("closing-tag pattern is valid")
        });
        let body = closing.replace_all(&body, |caps: &regex::Captures| {
            format!("<\\{}", &caps[0][1..])
        });
        let snapshot = if sections.is_empty() {
            CLEARED.to_owned()
        } else {
            format!("<system-reminder>\n{intro}\n\n{body}\n</system-reminder>")
        };
        let mut source = MessageSource::new(SOURCE);
        source.data.insert("form".into(), "snapshot".into());
        source.data.insert(
            "sections".into(),
            serde_json::to_value(sections).expect("sections are JSON"),
        );
        Some(create_user_message(NewUserMessage {
            content: vec![ContentBlock::text(snapshot)],
            source,
        }))
    }
}
