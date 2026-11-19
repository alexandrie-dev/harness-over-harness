//! SDK-facing JSON-RPC plugin. Stdout is reserved exclusively for protocol frames.
pub mod command_helpers;
pub mod error;
pub mod query_activity;
pub mod query_content;
pub mod query_input;
pub mod server;
mod shim;
pub mod transport;
pub mod user_question_helpers;
pub mod user_questions;

/// Temporary integration contracts for absent optional packages, not their implementations.
/// These reexports disappear when their owners provide the real types (PORT_NOTES.md).
pub mod integration {
    pub use crate::shim::{
        CommandImageAttachment, Commands, CommandsApi, DeepSeekFallback, DeepSeekFallbackApi,
        GoalActivationChanged, Goals, GoalsApi, ScheduleResolveAgent, SubagentRunEndInfo,
        Subagents, SubagentsApi,
    };
}
pub use server::{HarnessSdkJsonRpcServer, HarnessSdkJsonRpcServerOptions};

use hoh_agent::Agents;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_working_directory::WorkingDirectoryKey;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use tokio::io::{AsyncBufRead, AsyncWrite, BufReader};
use transport::{JsonRpcLineTransport, RequestHandler};

pub(crate) fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub(crate) fn error_string(error: &(dyn std::error::Error + 'static)) -> String {
    if let Some(e) = HarnessError::of(error) {
        format!("{}: {}", e.name(), e.message())
    } else if let Some(e) = error.downcast_ref::<error::SdkServerError>() {
        format!("{}: {e}", e.name())
    } else {
        format!("Error: {error}")
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcConfig {
    #[serde(default)]
    pub max_tokens_as_success: bool,
}
pub type Input = Box<dyn AsyncBufRead + Unpin + Send>;
pub type Output = Box<dyn AsyncWrite + Unpin + Send>;
/// Runtime-only source input/output/exit hooks, outside serialized plugin config.
pub struct JsonRpcRuntimeHooks {
    streams: Mutex<Option<(Input, Output)>>,
    exit: Arc<dyn Fn(i32) + Send + Sync>,
}
impl JsonRpcRuntimeHooks {
    pub fn new(input: Input, output: Output, exit: impl Fn(i32) + Send + Sync + 'static) -> Self {
        Self {
            streams: Mutex::new(Some((input, output))),
            exit: Arc::new(exit),
        }
    }
}
hoh_kernel::service!(pub JsonRpcRuntime: JsonRpcRuntimeHooks = "sdkJsonRpcRuntimeHooks");
#[derive(Clone, Debug, Serialize)]
pub struct SdkSessionActivation {
    pub ready: bool,
}
hoh_kernel::service!(pub SdkSessionActivationKey: SdkSessionActivation = "sdkSessionActivation");
/// Libraries cannot exit the process (PORTING §8). The app observes this after root disposal.
#[derive(Clone)]
pub struct SdkProcessExit {
    signal: tokio::sync::watch::Sender<Option<i32>>,
}
impl SdkProcessExit {
    pub async fn wait(&self) -> i32 {
        let mut signal = self.signal.subscribe();
        // This value owns its sender; the channel cannot close while wait is active.
        let code = signal.wait_for(Option::is_some).await;
        code.ok().and_then(|code| *code).unwrap_or(0)
    }
}
hoh_kernel::service!(pub SdkProcessExitKey: SdkProcessExit = "sdkProcessExit");
#[derive(Inject)]
pub struct Deps {
    _agents: Service<Agents>,
    _working_directory: Service<WorkingDirectoryKey>,
}
pub struct SdkJsonRpcServer;
impl Plugin for SdkJsonRpcServer {
    const NAME: &'static str = "sdk-jsonrpc-server";
    type Config = JsonRpcConfig;
    type Deps = Deps;
    async fn apply(ctx: Context, _deps: Deps, config: Cfg<JsonRpcConfig>) -> Result<(), BoxError> {
        let hooks = ctx.get::<JsonRpcRuntime>();
        let (input, output): (Input, Output) = match &hooks {
            Some(hooks) => lock(&hooks.streams)
                .take()
                .ok_or("SDK JSON-RPC streams were already consumed")?,
            None => (
                Box::new(BufReader::new(tokio::io::stdin())),
                Box::new(tokio::io::stdout()),
            ),
        };
        let transport = JsonRpcLineTransport::prepare(&ctx, output)?;
        let server = HarnessSdkJsonRpcServer::new(
            ctx.clone(),
            Arc::new(transport.clone()),
            HarnessSdkJsonRpcServerOptions {
                max_tokens_as_success: config.get().max_tokens_as_success,
            },
        )?;
        ctx.provide::<SdkSessionActivationKey>(Arc::new(SdkSessionActivation { ready: true }))
            .await?;
        let exit_signal = SdkProcessExit {
            signal: tokio::sync::watch::channel(None).0,
        };
        ctx.provide::<SdkProcessExitKey>(Arc::new(exit_signal.clone()))
            .await?;
        let handler_ctx = ctx.clone();
        let handler_server = server.clone();
        let handler: RequestHandler = Arc::new(move |method, params| {
            let (ctx, server) = (handler_ctx.clone(), handler_server.clone());
            Box::pin(async move {
                if method == "initialize"
                    && let Some(loader) = ctx.get::<hoh_loader::LoaderKey>()
                {
                    loader.settled().await;
                }
                server.handle_request(&method, params).await
            })
        });
        transport.on_request(handler);
        let exiting = AtomicBool::new(false);
        let root = ctx.root().fiber();
        let exit_transport = transport.clone();
        transport.after_response(Arc::new(move |method, succeeded| {
            if method != "shutdown" || !succeeded || exiting.swap(true, Ordering::SeqCst) {
                return;
            }
            let (transport, root, signal, hooks) = (
                exit_transport.clone(),
                root.clone(),
                exit_signal.clone(),
                hooks.clone(),
            );
            // setImmediate's process-lifetime continuation deliberately outlives plugin disposal.
            tokio::spawn(async move {
                // Let all request tasks admitted in the current input turn enqueue
                // their responses before the writer's flush barrier.
                tokio::task::yield_now().await;
                let _ = transport.flush().await;
                root.dispose().await;
                signal.signal.send_replace(Some(0));
                if let Some(hooks) = hooks {
                    (hooks.exit)(0);
                }
            });
        }));
        // Source starts stdin inside jsonrpc.serve only after onRequest is installed. Starting
        // here preserves that admission boundary while keeping teardown ordered as
        // server.shutdown() -> transport.close().
        transport.start_reader(&ctx, input)?;
        ctx.effect("jsonrpc.serve", move || {
            Undo::future(move || async move {
                let result = server.shutdown().await;
                transport.close();
                result.map(|_| ())
            })
        })?;
        Ok(())
    }
}
