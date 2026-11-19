//! Durable command vocabulary shared with command producers and UI adapters.

use futures_util::future::BoxFuture;
use hoh_agent::Agent;
use hoh_attachment::ImageMediaType;
use hoh_error::BoxError;
use hoh_llm::ContentBlock;
use hoh_session::SessionSeq;
use hoh_timeout::AbortSignal;
use serde::{Deserialize, Serialize};
use std::{future::Future, sync::Arc};

pub use crate::brand::{CommandDefinitionId, CommandId};

/// One browser-submitted command attachment: encoded image input or a staged file receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum CommandSubmitAttachment {
    Image {
        media_type: ImageMediaType,
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    File {
        receipt_id: String,
    },
}

/// Immutable metadata for a command's optional unstructured input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandInputDescriptor {
    pub hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<bool>,
}

impl CommandInputDescriptor {
    pub fn new(hint: impl Into<String>) -> Self {
        Self {
            hint: hint.into(),
            attachments: None,
        }
    }

    pub fn with_attachments(mut self) -> Self {
        self.attachments = Some(true);
        self
    }

    pub fn accepts_attachments(&self) -> bool {
        self.attachments == Some(true)
    }
}

/// Expected command outcome rendered directly by the dispatching UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CommandResult {
    Success {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "sourceEventSeq"
        )]
        source_event_seq: Option<SessionSeq>,
    },
    Error {
        text: String,
    },
}

impl CommandResult {
    pub fn success(text: Option<impl Into<String>>) -> Self {
        Self::Success {
            text: text.map(Into::into),
            source_event_seq: None,
        }
    }

    pub fn success_from(source_event_seq: SessionSeq, text: Option<impl Into<String>>) -> Self {
        Self::Success {
            text: text.map(Into::into),
            source_event_seq: Some(source_event_seq),
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self::Error { text: text.into() }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Success { .. } => "success",
            Self::Error { .. } => "error",
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Success { text, .. } => text.as_deref(),
            Self::Error { text } => Some(text),
        }
    }

    pub fn source_event_seq(&self) -> Option<SessionSeq> {
        match self {
            Self::Success {
                source_event_seq, ..
            } => *source_event_seq,
            Self::Error { .. } => None,
        }
    }
}

/// One settled command execution and its lifecycle pairing identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandExecution {
    pub command_id: CommandId,
    pub result: CommandResult,
}

/// Handler-free immutable command view returned to UI adapters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDescriptor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition_id: Option<CommandDefinitionId>,
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<CommandInputDescriptor>,
}

/// Who issued a command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CommandSource {
    User,
}

/// Invocation passed to one registered command handler.
#[derive(Clone)]
pub struct CommandInvocation {
    pub command_id: CommandId,
    pub agent: Arc<dyn Agent>,
    pub raw_input: String,
    pub attachments: Arc<[ContentBlock]>,
    pub signal: AbortSignal,
}

pub type CommandFuture = BoxFuture<'static, Result<CommandResult, BoxError>>;
pub type CommandHandler = Arc<dyn Fn(CommandInvocation) -> CommandFuture + Send + Sync>;

/// Plugin-owned command registration.
#[derive(Clone)]
pub struct CommandDefinition {
    pub definition_id: Option<CommandDefinitionId>,
    pub name: String,
    pub description: String,
    pub input: Option<CommandInputDescriptor>,
    pub record_input: Option<bool>,
    pub handler: CommandHandler,
}

impl CommandDefinition {
    pub fn new<F, Fut>(name: impl Into<String>, description: impl Into<String>, handler: F) -> Self
    where
        F: Fn(CommandInvocation) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<CommandResult, BoxError>> + Send + 'static,
    {
        Self {
            definition_id: None,
            name: name.into(),
            description: description.into(),
            input: None,
            record_input: None,
            handler: Arc::new(move |invocation| Box::pin(handler(invocation))),
        }
    }
}

/// Syntactically valid slash command before registry resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCommand {
    pub name: String,
    pub raw_input: String,
}
