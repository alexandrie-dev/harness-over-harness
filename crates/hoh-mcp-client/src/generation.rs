use crate::{
    Config,
    transport::{http_transport, stdio_transport},
    types::McpCloseResult,
};
use hoh_error::BoxError;
use hoh_timeout::AbortSignal;
use rmcp::{
    ClientHandler, ClientLifecycleMode, ServiceError,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ClientCapabilities, ClientConfig,
        Implementation, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult,
        PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams, ReadResourceResult,
        ServerNotification, SubscriptionFilter, Tool,
    },
    service::{
        NotificationContext, Peer, RoleClient, RunningService, serve_client_with_lifecycle_and_ct,
    },
    transport::IntoTransport,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct HarnessClient {
    tool_changes: watch::Sender<u64>,
}

impl HarnessClient {
    fn new() -> (Self, watch::Receiver<u64>) {
        let (tool_changes, receiver) = watch::channel(0);
        (Self { tool_changes }, receiver)
    }

    fn notify_tool_change(&self) {
        self.tool_changes.send_modify(|revision| {
            *revision = revision.wrapping_add(1);
        });
    }
}

impl ClientHandler for HarnessClient {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("harness-over-harness", env!("CARGO_PKG_VERSION")),
        )
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.notify_tool_change();
    }
}

type Session = RunningService<RoleClient, HarnessClient>;

#[derive(Clone)]
pub struct McpGeneration {
    session: Arc<Mutex<Option<Session>>>,
    subscription: Arc<Mutex<Option<JoinHandle<()>>>>,
    subscription_ended: Arc<AtomicBool>,
    tool_changes: Arc<Mutex<watch::Receiver<u64>>>,
    close_timeout_ms: u64,
    #[cfg(test)]
    close_pause: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    #[cfg(test)]
    appears_closed: bool,
    #[cfg(test)]
    forced_close_result: Option<McpCloseResult>,
}

type ConnectFuture = Pin<Box<dyn Future<Output = Result<McpGeneration, BoxError>> + Send>>;
type ConnectFn = dyn Fn(Config, AbortSignal, Option<u64>) -> ConnectFuture + Send + Sync;

#[derive(Clone)]
pub(crate) struct GenerationConnector(Arc<ConnectFn>);

impl Default for GenerationConnector {
    fn default() -> Self {
        Self(Arc::new(|config, signal, timeout_ms| {
            Box::pin(async move {
                McpGeneration::connect_with_control(&config, &signal, timeout_ms).await
            })
        }))
    }
}

impl GenerationConnector {
    pub(crate) async fn connect(
        &self,
        config: &Config,
        signal: &AbortSignal,
        timeout_ms: Option<u64>,
    ) -> Result<McpGeneration, BoxError> {
        (self.0)(config.clone(), signal.clone(), timeout_ms).await
    }

    #[cfg(test)]
    pub(crate) fn from_fn(
        connect: impl Fn(Config, AbortSignal, Option<u64>) -> ConnectFuture + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(connect))
    }
}

impl McpGeneration {
    async fn peer(&self) -> Result<Peer<RoleClient>, BoxError> {
        let session = self.session.lock().await;
        Ok(session.as_ref().ok_or_else(closed_error)?.peer().clone())
    }

    pub async fn connect(config: &Config) -> Result<Self, BoxError> {
        let controller = hoh_timeout::AbortController::new();
        Self::connect_with_control(config, &controller.signal(), None).await
    }

    pub async fn connect_with_control(
        config: &Config,
        signal: &AbortSignal,
        timeout_ms: Option<u64>,
    ) -> Result<Self, BoxError> {
        let started = std::time::Instant::now();
        let (handler, tool_changes) = HarnessClient::new();
        let mut session = match config {
            Config::Stdio { .. } => {
                serve_controlled(
                    handler.clone(),
                    stdio_transport(config)?,
                    signal,
                    timeout_ms,
                    config.close_timeout_ms(),
                )
                .await?
            }
            Config::StreamableHttp { .. } => {
                serve_controlled(
                    handler.clone(),
                    http_transport(config)?,
                    signal,
                    timeout_ms,
                    config.close_timeout_ms(),
                )
                .await?
            }
        };
        let subscription_ended = Arc::new(AtomicBool::new(false));
        let subscription_timeout = timeout_ms.map(|total| {
            total
                .saturating_sub(started.elapsed().as_millis() as u64)
                .max(1)
        });
        let subscription = match start_tool_subscription(
            &session,
            handler,
            subscription_ended.clone(),
            signal,
            subscription_timeout,
        )
        .await
        {
            Ok(subscription) => subscription,
            Err(error) => {
                let close = session
                    .close_with_timeout(Duration::from_millis(config.close_timeout_ms().max(1)))
                    .await;
                if !matches!(close, Ok(Some(_))) {
                    return Err(unclosed_error("MCP subscription cleanup did not settle"));
                }
                return Err(error);
            }
        };
        Ok(Self {
            session: Arc::new(Mutex::new(Some(session))),
            subscription: Arc::new(Mutex::new(subscription)),
            subscription_ended,
            tool_changes: Arc::new(Mutex::new(tool_changes)),
            close_timeout_ms: config.close_timeout_ms(),
            #[cfg(test)]
            close_pause: None,
            #[cfg(test)]
            appears_closed: false,
            #[cfg(test)]
            forced_close_result: None,
        })
    }

    pub async fn list_tools_page(
        &self,
        cursor: Option<String>,
    ) -> Result<ListToolsResult, BoxError> {
        self.peer()
            .await?
            .list_tools(
                cursor.map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor))),
            )
            .await
            .map_err(|error| Box::new(error) as BoxError)
    }

    pub async fn list_all_tools(&self) -> Result<Vec<Tool>, BoxError> {
        if !self.has_tools().await {
            return Ok(Vec::new());
        }
        let mut tools = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.list_tools_page(cursor).await?;
            tools.extend(page.tools);
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(tools);
            }
        }
    }

    pub async fn call_tool(
        &self,
        name: String,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, BoxError> {
        match self
            .peer()
            .await?
            .call_tool_once(CallToolRequestParams::new(name).with_arguments(arguments))
            .await
            .map_err(|error| Box::new(error) as BoxError)?
        {
            CallToolResponse::Complete(result) => Ok(result),
            _ => Err(Box::new(ServiceError::UnexpectedResponse)),
        }
    }

    pub async fn list_resources(
        &self,
        params: Option<PaginatedRequestParams>,
    ) -> Result<ListResourcesResult, BoxError> {
        self.peer()
            .await?
            .list_resources(params)
            .await
            .map_err(|error| Box::new(error) as BoxError)
    }

    pub async fn list_resource_templates(
        &self,
        params: Option<PaginatedRequestParams>,
    ) -> Result<ListResourceTemplatesResult, BoxError> {
        self.peer()
            .await?
            .list_resource_templates(params)
            .await
            .map_err(|error| Box::new(error) as BoxError)
    }

    pub async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, BoxError> {
        self.peer()
            .await?
            .read_resource(params)
            .await
            .map_err(|error| Box::new(error) as BoxError)
    }

    pub async fn server_info(&self) -> Option<crate::types::McpServerInfo> {
        let session = self.session.lock().await;
        let info = session.as_ref()?.peer().peer_info()?;
        let server = info.server_info.as_ref()?;
        Some(crate::types::McpServerInfo {
            name: server.name.clone(),
            version: server.version.clone(),
        })
    }

    pub async fn has_tools(&self) -> bool {
        let session = self.session.lock().await;
        session
            .as_ref()
            .and_then(|session| session.peer().peer_info())
            .is_some_and(|info| info.capabilities.tools.is_some())
    }

    pub async fn has_resources(&self) -> bool {
        let session = self.session.lock().await;
        session
            .as_ref()
            .and_then(|session| session.peer().peer_info())
            .is_some_and(|info| info.capabilities.resources.is_some())
    }

    pub async fn instructions(&self) -> Option<String> {
        let session = self.session.lock().await;
        session.as_ref()?.peer().peer_info()?.instructions.clone()
    }

    pub async fn is_closed(&self) -> bool {
        #[cfg(test)]
        if self.appears_closed {
            return true;
        }
        self.session
            .lock()
            .await
            .as_ref()
            .is_none_or(RunningService::is_closed)
    }

    pub async fn wait_tool_change(&self) -> bool {
        let mut changes = self.tool_changes.lock().await;
        changes.changed().await.is_ok()
    }

    pub fn subscription_ended(&self) -> bool {
        self.subscription_ended.load(Ordering::Acquire)
    }

    pub async fn close(&self) -> McpCloseResult {
        if let Some(subscription) = self.subscription.lock().await.take() {
            subscription.abort();
        }
        let session = { self.session.lock().await.take() };
        let Some(mut session) = session else {
            return McpCloseResult {
                close_confirmed: true,
                cleanup_error: None,
            };
        };
        #[cfg(test)]
        if let Some((started, release)) = &self.close_pause {
            started.notify_one();
            release.notified().await;
        }
        let result = classify_close_result(
            session
                .close_with_timeout(Duration::from_millis(self.close_timeout_ms.max(1)))
                .await,
            self.close_timeout_ms,
        );
        #[cfg(test)]
        if let Some(forced) = &self.forced_close_result {
            return forced.clone();
        }
        result
    }

    #[cfg(test)]
    pub(crate) async fn from_test_transport(
        transport: tokio::io::DuplexStream,
        appears_closed: bool,
        forced_close_result: Option<McpCloseResult>,
    ) -> Self {
        use rmcp::ServiceExt;

        let (handler, tool_changes) = HarnessClient::new();
        let session = handler.serve(transport).await.unwrap();
        Self {
            session: Arc::new(Mutex::new(Some(session))),
            subscription: Arc::new(Mutex::new(None)),
            subscription_ended: Arc::new(AtomicBool::new(false)),
            tool_changes: Arc::new(Mutex::new(tool_changes)),
            close_timeout_ms: 1_000,
            close_pause: None,
            appears_closed,
            forced_close_result,
        }
    }
}

fn classify_close_result<T, E: std::fmt::Display>(
    result: Result<Option<T>, E>,
    close_timeout_ms: u64,
) -> McpCloseResult {
    match result {
        Ok(Some(_)) => McpCloseResult {
            close_confirmed: true,
            cleanup_error: None,
        },
        Ok(None) => McpCloseResult {
            close_confirmed: false,
            cleanup_error: Some(format!("MCP cleanup exceeded {close_timeout_ms}ms")),
        },
        Err(error) => McpCloseResult {
            close_confirmed: false,
            cleanup_error: Some(error.to_string()),
        },
    }
}

async fn serve_controlled<T, E, A>(
    handler: HarnessClient,
    transport: T,
    signal: &AbortSignal,
    timeout_ms: Option<u64>,
    close_timeout_ms: u64,
) -> Result<Session, BoxError>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let cancellation = CancellationToken::new();
    let serving =
        serve_client_with_lifecycle_and_ct(handler, transport, lifecycle(), cancellation.clone());
    tokio::pin!(serving);
    let deadline = async {
        if let Some(timeout_ms) = timeout_ms {
            tokio::time::sleep(Duration::from_millis(timeout_ms.max(1))).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(deadline);
    let interrupted = tokio::select! {
        result = &mut serving => return result.map_err(|error| Box::new(error) as BoxError),
        reason = signal.cancelled() => Some(Box::new(reason) as BoxError),
        _ = &mut deadline => Some(Box::new(crate::diagnostic::McpLocalError::new(
            format!("MCP connection timed out after {}ms", timeout_ms.unwrap_or_default()),
            crate::diagnostic::McpLocalErrorCode::Timeout,
        )) as BoxError),
    };
    cancellation.cancel();
    if tokio::time::timeout(Duration::from_millis(close_timeout_ms.max(1)), &mut serving)
        .await
        .is_err()
    {
        return Err(unclosed_error("MCP connection cleanup did not settle"));
    }
    Err(interrupted.expect("select interruption always carries an error"))
}

fn lifecycle() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        legacy_version: None,
    }
}

async fn start_tool_subscription(
    session: &Session,
    handler: HarnessClient,
    ended: Arc<AtomicBool>,
    signal: &AbortSignal,
    timeout_ms: Option<u64>,
) -> Result<Option<JoinHandle<()>>, BoxError> {
    let Some(info) = session.peer().peer_info() else {
        return Ok(None);
    };
    if info.protocol_version < ProtocolVersion::V_2026_07_28
        || info
            .capabilities
            .tools
            .as_ref()
            .is_none_or(|tools| tools.list_changed != Some(true))
    {
        return Ok(None);
    }
    let listen = session
        .peer()
        .listen(SubscriptionFilter::builder().tools_list_changed().build());
    tokio::pin!(listen);
    let deadline = async {
        if let Some(timeout_ms) = timeout_ms {
            tokio::time::sleep(Duration::from_millis(timeout_ms.max(1))).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(deadline);
    let mut subscription = tokio::select! {
        result = &mut listen => result.map_err(|error| Box::new(error) as BoxError)?,
        reason = signal.cancelled() => return Err(Box::new(reason)),
        _ = &mut deadline => return Err(Box::new(crate::diagnostic::McpLocalError::new(
            format!("MCP subscription timed out after {}ms", timeout_ms.unwrap_or_default()),
            crate::diagnostic::McpLocalErrorCode::Timeout,
        ))),
    };
    Ok(Some(tokio::spawn(async move {
        loop {
            match subscription.next().await {
                Ok(Some(ServerNotification::ToolListChangedNotification(_))) => {
                    handler.notify_tool_change();
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        ended.store(true, Ordering::Release);
        handler.notify_tool_change();
    })))
}

fn unclosed_error(message: &str) -> BoxError {
    Box::new(crate::diagnostic::McpLocalError::new(
        message,
        crate::diagnostic::McpLocalErrorCode::UnclosedGeneration,
    ))
}

fn closed_error() -> BoxError {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::NotConnected,
        "MCP connection is closed",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::{
        ClientServiceExt, ServerHandler, ServiceExt,
        model::{
            CallToolResponse, ContentBlock, ErrorData, ListToolsResult, ServerCapabilities,
            ServerConfig, SubscriptionFilter,
        },
        service::{RequestContext, RoleServer, SubscriptionContext},
    };

    #[derive(Clone)]
    struct FixtureServer;

    #[derive(Clone)]
    struct ChangingServer;

    impl ServerHandler for ChangingServer {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_tool_list_changed()
                    .build(),
            )
        }

        fn accepted_subscription_filter(
            &self,
            requested: &SubscriptionFilter,
        ) -> Option<SubscriptionFilter> {
            Some(requested.supported_by(&self.get_info().capabilities))
        }

        async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
            context
                .sink()
                .notify_tool_list_changed()
                .await
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
            Ok(())
        }
    }

    impl ServerHandler for FixtureServer {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
                .with_instructions("fixture instructions")
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            let mut result = ListToolsResult::default();
            result.tools.push(Tool::new(
                "echo",
                "Echo input",
                serde_json::Map::from_iter([("type".into(), serde_json::json!("object"))]),
            ));
            Ok(result)
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            let text = request
                .arguments
                .and_then(|args| args.get("text").cloned())
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
        }
    }

    #[tokio::test]
    async fn in_memory_generation_negotiates_lists_calls_and_closes() {
        let (server_transport, client_transport) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            FixtureServer
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap()
        });
        let (handler, tool_changes) = HarnessClient::new();
        let session = handler.serve(client_transport).await.unwrap();
        let generation = McpGeneration {
            session: Arc::new(Mutex::new(Some(session))),
            subscription: Arc::new(Mutex::new(None)),
            subscription_ended: Arc::new(AtomicBool::new(false)),
            tool_changes: Arc::new(Mutex::new(tool_changes)),
            close_timeout_ms: 1_000,
            close_pause: None,
            appears_closed: false,
            forced_close_result: None,
        };
        assert_eq!(
            generation.instructions().await.as_deref(),
            Some("fixture instructions")
        );
        let tools = generation.list_all_tools().await.unwrap();
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_ref())
                .collect::<Vec<_>>(),
            ["echo"]
        );
        let result = generation
            .call_tool(
                "echo".into(),
                serde_json::Map::from_iter([("text".into(), serde_json::json!("hello"))]),
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&result.content[0]).unwrap()["text"],
            "hello"
        );
        assert!(generation.close().await.close_confirmed);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn modern_tool_list_subscription_reaches_generation() {
        let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
        let server = tokio::spawn(async move {
            ChangingServer
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap()
        });
        let (handler, tool_changes) = HarnessClient::new();
        let session = handler
            .clone()
            .serve_with_lifecycle(client_transport, lifecycle())
            .await
            .unwrap();
        let subscription_ended = Arc::new(AtomicBool::new(false));
        let controller = hoh_timeout::AbortController::new();
        let subscription = start_tool_subscription(
            &session,
            handler,
            subscription_ended.clone(),
            &controller.signal(),
            Some(2_000),
        )
        .await
        .unwrap();
        let generation = McpGeneration {
            session: Arc::new(Mutex::new(Some(session))),
            subscription: Arc::new(Mutex::new(subscription)),
            subscription_ended,
            tool_changes: Arc::new(Mutex::new(tool_changes)),
            close_timeout_ms: 1_000,
            close_pause: None,
            appears_closed: false,
            forced_close_result: None,
        };
        assert!(
            tokio::time::timeout(Duration::from_secs(2), generation.wait_tool_change())
                .await
                .unwrap()
        );
        assert!(generation.close().await.close_confirmed);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn close_does_not_hold_the_session_mutex_while_waiting() {
        let (server_transport, client_transport) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            FixtureServer
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap()
        });
        let (handler, tool_changes) = HarnessClient::new();
        let session = handler.serve(client_transport).await.unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let generation = McpGeneration {
            session: Arc::new(Mutex::new(Some(session))),
            subscription: Arc::new(Mutex::new(None)),
            subscription_ended: Arc::new(AtomicBool::new(false)),
            tool_changes: Arc::new(Mutex::new(tool_changes)),
            close_timeout_ms: 1_000,
            close_pause: Some((started.clone(), release.clone())),
            appears_closed: false,
            forced_close_result: None,
        };
        let closing = {
            let generation = generation.clone();
            tokio::spawn(async move { generation.close().await })
        };
        started.notified().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), generation.is_closed())
                .await
                .unwrap()
        );
        release.notify_one();
        assert!(closing.await.unwrap().close_confirmed);
        server.await.unwrap();
    }

    #[test]
    fn close_timeout_is_classified_as_unconfirmed_cleanup() {
        let result = classify_close_result::<(), std::io::Error>(Ok(None), 17);
        assert!(!result.close_confirmed);
        assert_eq!(
            result.cleanup_error.as_deref(),
            Some("MCP cleanup exceeded 17ms")
        );
    }
}
