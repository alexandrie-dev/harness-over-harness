//! Example: record a compact summary after a session turn ends.
use hoh_kernel::{Cfg, Context, Error, Inject, NoConfig, Plugin, Service};
use hoh_llm::ContentBlock;
use hoh_session::{Session, SessionEventEmitted, append_plugin_record};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolContextUnavailableError, ToolRegistrationOptions,
    Tools, define_tool,
};
use serde_json::{Value, json};
use std::sync::{Arc, mpsc};

const RECORD: &str = "plugin:turn-note/turn-ended";

#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
}
pub struct AuthorPlugin;

pub fn last_turn(session: &Session) -> Option<Value> {
    session
        .snapshot_events(None, None)
        .iter()
        .rev()
        .find(|e| e.event_type() == RECORD)
        .map(|e| e.data().clone())
}

fn record(session: &Session, end: &Value) -> Result<(), Error> {
    let binding = session
        .snapshot_events(None, None)
        .iter()
        .rev()
        .find(|e| e.event_type() == "plugin:external-harness/session")
        .and_then(|e| {
            e.data()
                .get("harness")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    let engine = match binding.as_deref() {
        Some("claude") => "claude",
        Some("codex") => "codex",
        Some(_) => "other",
        None => "hoh",
    };
    let outcome = match end["reason"]["kind"].as_str() {
        Some("completed") => "completed",
        Some("aborted") => "stopped",
        Some("error") => "failed",
        _ => "unknown",
    };
    append_plugin_record(
        session,
        RECORD,
        json!({"turn": end["turn"], "engine": engine, "outcome": outcome}),
    )?;
    Ok(())
}

impl Plugin for AuthorPlugin {
    const NAME: &'static str = "hoh:plugin-turn-note";
    type Config = NoConfig;
    type Deps = Deps;
    async fn apply(ctx: Context, deps: Deps, _: Cfg<NoConfig>) -> Result<(), Error> {
        let output = DefineToolOutput {
            schema: json!({"type":"object","additionalProperties":false,"properties":{"text":{"type":"string","required":true}}}),
            render: Arc::new(|_, value| {
                vec![ContentBlock::text(value["text"].as_str().unwrap_or(""))]
            }),
            presentation_meta: None,
        };
        let tool = DefineToolOptions::new(
            "last_turn",
            "Read the last finished turn in the current session.",
            json!({}),
            output,
            |_, exec| {
                Box::pin(async move {
                    let agent = exec.agent.as_ref().ok_or_else(|| {
                        ToolContextUnavailableError::new("last_turn requires an agent session")
                    })?;
                    let text = match last_turn(agent.session()) {
                        Some(note) => format!(
                            "Turn {} · {} · {}",
                            note["turn"],
                            note["engine"].as_str().unwrap_or("other"),
                            note["outcome"].as_str().unwrap_or("unknown")
                        ),
                        None => "No finished turns yet".to_owned(),
                    };
                    Ok(json!({"text": text}))
                })
            },
        );
        deps.tools
            .register(&ctx, define_tool(tool)?, ToolRegistrationOptions::default())?;
        let (sender, receiver) = mpsc::sync_channel::<(Session, Value)>(128);
        std::thread::Builder::new()
            .name("turn-note".into())
            .spawn(move || {
                while let Ok((session, end)) = receiver.recv() {
                    if let Err(error) = record(&session, &end) {
                        eprintln!("turn-note: cannot append record: {error}");
                    }
                }
            })?;
        let logger = ctx.logger();
        ctx.on_contained::<SessionEventEmitted>(move |event| {
            if event.event.event_type() == "turn/end"
                && sender
                    .try_send((event.session.clone(), event.event.data().clone()))
                    .is_err()
            {
                logger.warn(format_args!(
                    "turn-note: observation queue is full or closed"
                ));
            }
            Ok(())
        })?;
        Ok(())
    }
}
