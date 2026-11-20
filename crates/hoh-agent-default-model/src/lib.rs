//! Default-model selection for agents without a session-specific selection.

use hoh_agent::ModelSelection;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, Inject, KernelError, Plugin, Service, Undo, Volatile};
use hoh_loader::{LoaderKey, config_editor::ConfigEditorKey};
use hoh_settings::{Settings, SettingsPresentation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::{mpsc, oneshot};

type SaveFuture = Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send + 'static>>;

/// Default model selection supplied by plugin configuration.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Registered provider route.
    pub provider: Volatile<String>,
    /// Provider-owned model id.
    pub model: Volatile<String>,
    /// Adapter-owned reasoning effort; omission follows the provider default.
    #[serde(default, skip_serializing_if = "reasoning_effort_unset")]
    pub reasoning_effort: Volatile<Option<String>>,
}

fn reasoning_effort_unset(value: &Volatile<Option<String>>) -> bool {
    value.0.is_none()
}

fn selection_value(selection: &ModelSelection) -> Value {
    let mut config = Map::new();
    config.insert("provider".into(), Value::String(selection.provider.clone()));
    config.insert("model".into(), Value::String(selection.model.clone()));
    if let Some(reasoning_effort) = &selection.reasoning_effort {
        config.insert(
            "reasoningEffort".into(),
            Value::String(reasoning_effort.as_str().into()),
        );
    }
    Value::Object(config)
}

struct SaveRequest {
    job: SaveFuture,
    reply: oneshot::Sender<Result<(), BoxError>>,
}

#[derive(Clone)]
struct SaveQueue {
    sender: mpsc::UnboundedSender<SaveRequest>,
}

impl SaveQueue {
    fn start(ctx: &Context) -> Result<Self, KernelError> {
        let (sender, mut receiver) = mpsc::unbounded_channel::<SaveRequest>();
        let lifetime = Arc::new(std::sync::Mutex::new(None::<Disposer>));
        let worker_lifetime = lifetime.clone();
        let disposer = ctx
            .root()
            .spawn("agent default model saves", move |cancel| async move {
                loop {
                    let request = tokio::select! {
                        _ = cancel.cancelled() => break,
                        request = receiver.recv() => match request {
                            Some(request) => request,
                            None => break,
                        },
                    };
                    let result = tokio::select! {
                        _ = cancel.cancelled() => {
                            let _ = request.reply.send(Err(closed_error()));
                            break;
                        }
                        result = request.job => result,
                    };
                    let _ = request.reply.send(result);
                }
                if let Some(disposer) = worker_lifetime
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    // `dispose()` starts cleanup synchronously. Dropping its wait future lets this
                    // worker return, after which the root effect's join cleanup can complete.
                    drop(disposer.dispose());
                }
                Ok(())
            })?;
        *lifetime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(disposer);
        Ok(Self { sender })
    }

    fn enqueue(&self, job: SaveFuture) -> SaveFuture {
        let (reply, result) = oneshot::channel();
        if self.sender.send(SaveRequest { job, reply }).is_err() {
            return Box::pin(std::future::ready(Err(closed_error())));
        }
        Box::pin(async move {
            result
                .await
                .map_err(|_| Box::new(KernelError::Closed) as BoxError)?
        })
    }
}

fn closed_error() -> BoxError {
    Box::new(KernelError::Closed)
}

#[derive(Inject)]
struct SettingsDeps {
    settings: Service<Settings>,
}

/// Owns the live default selection independently of any host or transport.
#[derive(Clone)]
pub struct AgentDefaultModelConfig {
    owner_context: Context,
    config: Cfg<Config>,
    saves: SaveQueue,
}

hoh_kernel::service!(pub AgentDefaultModel: AgentDefaultModelConfig = "agentDefaultModel");

impl AgentDefaultModelConfig {
    /// Read the current detached default selection.
    pub fn current_selection(&self) -> ModelSelection {
        let config = self.config.get();
        ModelSelection {
            provider: config.provider.0.clone(),
            model: config.model.0.clone(),
            reasoning_effort: config.reasoning_effort.0.clone().map(Into::into),
        }
    }

    /// Save the complete selection through the caller-visible configuration editor.
    ///
    /// The request enters the FIFO before this method returns its future, matching the
    /// source Promise tail's call-submission order. Without an owning entry or an editor,
    /// saving is a no-op.
    pub fn save_selection(
        &self,
        caller: &Context,
        next: ModelSelection,
    ) -> impl Future<Output = Result<(), BoxError>> + Send + 'static {
        let Some(entry_id) = self.owner_context.fiber().entry_id() else {
            return Box::pin(std::future::ready(Ok(()))) as SaveFuture;
        };
        let Some(loader) = self.owner_context.get::<LoaderKey>() else {
            return Box::pin(std::future::ready(Ok(()))) as SaveFuture;
        };
        let Some(entry) = loader.resolve(&entry_id) else {
            return Box::pin(std::future::ready(Ok(()))) as SaveFuture;
        };
        let Some(editor) = caller.get::<ConfigEditorKey>() else {
            return Box::pin(std::future::ready(Ok(()))) as SaveFuture;
        };
        let config = selection_value(&next);
        self.saves.enqueue(Box::pin(async move {
            editor.edit(&entry, move |_, _| config).await?;
            Ok(())
        }))
    }
}

impl Plugin for AgentDefaultModelConfig {
    const NAME: &'static str = "AgentDefaultModelConfig";
    type Config = Config;
    type Deps = ();

    async fn apply(ctx: Context, _: (), config: Cfg<Config>) -> Result<(), BoxError> {
        let service = Arc::new(Self {
            owner_context: ctx.clone(),
            config,
            saves: SaveQueue::start(&ctx)?,
        });
        ctx.provide::<AgentDefaultModel>(service).await?;

        let owner = ctx.fiber();
        ctx.inject::<SettingsDeps, _>(move |child, deps| {
            let owner = owner.clone();
            async move {
                child.try_effect(
                    "settings.configure()",
                    move |undos| -> Result<(), BoxError> {
                        let undo = deps
                            .settings
                            .configure(SettingsPresentation { auto: Some(false) }, owner)?;
                        undos.push(Undo::sync(undo))?;
                        Ok(())
                    },
                )?;
                Ok(())
            }
        })?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/agent_default_model.rs"]
mod tests;
