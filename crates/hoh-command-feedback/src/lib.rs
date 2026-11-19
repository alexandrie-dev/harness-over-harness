//! Session feedback producer, live-session service, and `/feedback` command.

mod types;

pub use types::*;

use hoh_anonymous_user_id::{AnonymousUserIdOptions, AnonymousUserIdStore};
use hoh_commands::{
    CommandDefinition, CommandDefinitionId, CommandFuture, CommandInputDescriptor,
    CommandInvocation, CommandResult, Commands,
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Plugin, Service};
use hoh_session::{Session, Sessions};
use std::sync::Arc;

pub const NAME: &str = "command-feedback";
const DEFINITION_ID: &str = "@deepseek-ai/dsh-command-feedback";
const COMMAND_NAME: &str = "feedback";
const DESCRIPTION: &str = "Record feedback about this session";
const INPUT_HINT: &str = "<text>";
const USAGE: &str = "Usage: /feedback <text>";

pub const FEEDBACK_CATEGORIES: [FeedbackCategory; 7] = [
    FeedbackCategory::TaskResult,
    FeedbackCategory::InstructionFollowing,
    FeedbackCategory::ProductInteraction,
    FeedbackCategory::ServiceStability,
    FeedbackCategory::ResourceCost,
    FeedbackCategory::SecurityPrivacyPermission,
    FeedbackCategory::Other,
];

hoh_kernel::service!(AnonymousUserIds: AnonymousUserIdStore = "anonymousUserIdStore");

async fn user_id_store(ctx: &Context) -> Result<Service<AnonymousUserIds>, BoxError> {
    if ctx.get::<AnonymousUserIds>().is_none() {
        ctx.root()
            .provide::<AnonymousUserIds>(Arc::new(AnonymousUserIdStore::default()))
            .await?;
    }
    ctx.get::<AnonymousUserIds>()
        .ok_or_else(|| "command-feedback: anonymous user id store is unavailable".into())
}

/// Append one authoritative log-only feedback event.
pub fn record_feedback(session: &Session, entry: FeedbackRecord) -> Result<(), BoxError> {
    let text = entry
        .text
        .as_deref()
        .map(hoh_util_values::js_trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    session.append(
        "feedback/record",
        serde_json::to_value(FeedbackRecord {
            text,
            category: entry.category,
        })?,
        None,
    )?;
    Ok(())
}

fn execute_feedback_command(
    invocation: CommandInvocation,
    user_ids: Service<AnonymousUserIds>,
) -> CommandFuture {
    if hoh_util_values::js_trim(&invocation.raw_input).is_empty() {
        return Box::pin(async {
            Ok(CommandResult::error(format!(
                "Feedback text is required. {USAGE}"
            )))
        });
    }
    if let Err(error) = record_feedback(
        invocation.agent.session(),
        FeedbackRecord {
            text: Some(invocation.raw_input),
            category: None,
        },
    ) {
        return Box::pin(async move { Err(error) });
    }
    let session_id = invocation.agent.session().id().clone();
    Box::pin(async move {
        let user_id = tokio::task::spawn_blocking(move || {
            user_ids.get_or_create_anonymous_user_id(AnonymousUserIdOptions::default())
        })
        .await??;
        Ok(CommandResult::success(Some(format!(
            "Feedback recorded for session {session_id}\nAnonymous user: {user_id}."
        ))))
    })
}

#[derive(Clone)]
pub struct SessionFeedbackService {
    sessions: Service<Sessions>,
}

hoh_kernel::service!(pub SessionFeedback: SessionFeedbackService = "sessionFeedback");

impl SessionFeedbackService {
    /// Remote: sessionFeedback.record
    pub fn record(
        &self,
        request: SessionFeedbackRecordRequest,
    ) -> std::future::Ready<Result<SessionFeedbackRecordResult, BoxError>> {
        let result = (|| {
            let Some(session) = self.sessions.get(&request.session_id) else {
                return Ok(SessionFeedbackRecordResult::session_not_found(
                    request.session_id,
                ));
            };
            record_feedback(
                &session,
                FeedbackRecord {
                    text: request.text,
                    category: request.category,
                },
            )?;
            Ok(SessionFeedbackRecordResult::recorded())
        })();
        std::future::ready(result)
    }
}

#[derive(Inject)]
pub struct SessionFeedbackDeps {
    sessions: Service<Sessions>,
}

impl Plugin for SessionFeedbackService {
    const NAME: &'static str = "session-feedback-service";
    type Config = NoConfig;
    type Deps = SessionFeedbackDeps;

    async fn apply(
        ctx: Context,
        deps: Self::Deps,
        _config: Cfg<Self::Config>,
    ) -> Result<(), BoxError> {
        ctx.provide::<SessionFeedback>(Arc::new(Self {
            sessions: deps.sessions,
        }))
        .await?;
        Ok(())
    }
}

#[derive(Inject)]
pub struct Deps {
    commands: Service<Commands>,
}

pub struct CommandFeedback;

impl Plugin for CommandFeedback {
    const NAME: &'static str = NAME;
    type Config = NoConfig;
    type Deps = Deps;

    async fn apply(
        ctx: Context,
        deps: Self::Deps,
        _config: Cfg<Self::Config>,
    ) -> Result<(), BoxError> {
        let user_ids = user_id_store(&ctx).await?;
        ctx.plugin::<SessionFeedbackService>(serde_json::json!({}))?;

        let mut definition = CommandDefinition::new(COMMAND_NAME, DESCRIPTION, move |invocation| {
            execute_feedback_command(invocation, user_ids.clone())
        });
        definition.definition_id = Some(CommandDefinitionId::from(DEFINITION_ID));
        definition.input = Some(CommandInputDescriptor::new(INPUT_HINT));
        definition.record_input = Some(false);
        deps.commands.register(&ctx, definition)?;
        Ok(())
    }
}
