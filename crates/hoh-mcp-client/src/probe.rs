use crate::{
    Config,
    credentials::McpTemplateBinding,
    diagnostic::describe_error,
    generation::GenerationConnector,
    types::{McpProbeOutcome, McpProbeResult, McpProbeStage},
};
use hoh_credentials::CredentialProvider;
use hoh_error::BoxError;
use hoh_timeout::{AbortController, AbortSignal};
use std::{future::Future, time::Instant};

#[derive(Clone, Debug)]
pub struct McpProbeOptions {
    pub timeout_ms: u64,
    pub max_pages: usize,
    pub max_tool_names: usize,
    pub signal: Option<AbortSignal>,
}
impl Default for McpProbeOptions {
    fn default() -> Self {
        Self {
            timeout_ms: 10_000,
            max_pages: 16,
            max_tool_names: 512,
            signal: None,
        }
    }
}

pub async fn probe_server(
    provider: Option<std::sync::Arc<dyn CredentialProvider>>,
    config: Config,
    options: McpProbeOptions,
) -> Result<McpProbeResult, BoxError> {
    probe_server_with_connector(provider, config, options, GenerationConnector::default()).await
}

async fn probe_server_with_connector(
    provider: Option<std::sync::Arc<dyn CredentialProvider>>,
    config: Config,
    options: McpProbeOptions,
    connector: GenerationConnector,
) -> Result<McpProbeResult, BoxError> {
    config
        .validate()
        .map_err(|error| -> BoxError { error.into() })?;
    if options.timeout_ms == 0 {
        return Err("probe timeoutMs must be a positive integer".into());
    }
    if options.max_pages == 0 {
        return Err("probe maxPages must be a positive integer".into());
    }
    if options.max_tool_names == 0 {
        return Err("probe maxToolNames must be a positive integer".into());
    }
    let started = Instant::now();
    let controller = AbortController::new();
    let signal = options
        .signal
        .clone()
        .unwrap_or_else(|| controller.signal());
    if signal.aborted() {
        return Ok(failure(
            McpProbeStage::Connect,
            "probe aborted",
            started,
            None,
        ));
    }
    let binding = McpTemplateBinding::new(provider, config)?;
    let resolved = match bounded(
        &signal,
        options.timeout_ms,
        started,
        binding.resolve(&signal),
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            let message = probe_connect_error(&signal, &binding, error.as_ref());
            return Ok(failure(McpProbeStage::Connect, &message, started, None));
        }
        Err(message) => {
            return Ok(failure(McpProbeStage::Connect, message, started, None));
        }
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    if elapsed_ms >= options.timeout_ms {
        return Ok(failure(
            McpProbeStage::Connect,
            "MCP probe timed out",
            started,
            None,
        ));
    }
    let generation = match connector
        .connect(&resolved, &signal, Some(options.timeout_ms - elapsed_ms))
        .await
    {
        Ok(generation) => generation,
        Err(error) => {
            let message = probe_connect_error(&signal, &binding, error.as_ref());
            let cleanup_unconfirmed = hoh_error::HarnessError::of(error.as_ref())
                .is_some_and(|error| error.code() == "MCP_UNCLOSED_GENERATION");
            return Ok(failure(
                McpProbeStage::Connect,
                &message,
                started,
                cleanup_unconfirmed.then(|| crate::types::McpCloseResult {
                    close_confirmed: false,
                    cleanup_error: Some(message.clone()),
                }),
            ));
        }
    };
    let server_info = generation.server_info().await;
    let mut names = Vec::new();
    let mut cursor = None;
    let mut truncated = false;
    let list_result: Result<(), BoxError> = async {
        if !generation.has_tools().await {
            return Ok(());
        }
        for _ in 0..options.max_pages {
            let page = bounded(
                &signal,
                options.timeout_ms,
                started,
                generation.list_tools_page(cursor.take()),
            )
            .await
            .map_err(|message| -> BoxError { message.into() })??;
            for tool in page.tools {
                if names.len() == options.max_tool_names {
                    truncated = true;
                    break;
                }
                names.push(tool.name.into_owned());
            }
            if truncated {
                return Ok(());
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(());
            }
        }
        truncated = cursor.is_some();
        Ok(())
    }
    .await;
    let duration_ms = started.elapsed().as_millis() as u64;
    let close = generation.close().await;
    let outcome = match list_result {
        Ok(()) => McpProbeOutcome::Success {
            server_info,
            tool_names: names,
            truncated,
            duration_ms,
        },
        Err(error) => McpProbeOutcome::Failure {
            stage: McpProbeStage::List,
            message: binding.redact(&describe_error(error.as_ref())),
            duration_ms,
        },
    };
    Ok(McpProbeResult {
        outcome,
        close_confirmed: close.close_confirmed,
        cleanup_error: close.cleanup_error,
    })
}

fn probe_connect_error(
    signal: &AbortSignal,
    binding: &McpTemplateBinding,
    error: &(dyn std::error::Error + 'static),
) -> String {
    if signal.aborted() {
        "probe aborted".to_owned()
    } else {
        binding.redact(&describe_error(error))
    }
}

async fn bounded<T>(
    signal: &AbortSignal,
    budget_ms: u64,
    started: Instant,
    future: impl Future<Output = T>,
) -> Result<T, &'static str> {
    let elapsed_ms = started.elapsed().as_millis() as u64;
    if elapsed_ms >= budget_ms {
        return Err("MCP probe timed out");
    }
    let remaining_ms = budget_ms - elapsed_ms;
    tokio::select! {
        _ = signal.cancelled() => Err("probe aborted"),
        result = tokio::time::timeout(std::time::Duration::from_millis(remaining_ms.max(1)), future) => {
            result.map_err(|_| "MCP probe timed out")
        }
    }
}

fn failure(
    stage: McpProbeStage,
    message: &str,
    started: Instant,
    close: Option<crate::types::McpCloseResult>,
) -> McpProbeResult {
    let close = close.unwrap_or(crate::types::McpCloseResult {
        close_confirmed: true,
        cleanup_error: None,
    });
    McpProbeResult {
        outcome: McpProbeOutcome::Failure {
            stage,
            message: message.to_owned(),
            duration_ms: started.elapsed().as_millis() as u64,
        },
        close_confirmed: close.close_confirmed,
        cleanup_error: close.cleanup_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::McpGeneration;
    use rmcp::{
        ServerHandler, ServiceExt,
        model::{
            ErrorData, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
            Tool,
        },
        service::{RequestContext, RoleServer},
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Clone)]
    struct PagingServer {
        calls: Arc<AtomicUsize>,
    }

    impl ServerHandler for PagingServer {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn list_tools(
            &self,
            request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let cursor = request.and_then(|request| request.cursor);
            let mut result = ListToolsResult::default();
            if cursor.is_none() {
                result.tools.push(tool("one"));
                result.next_cursor = Some("second".into());
            } else {
                result.tools.push(tool("two"));
                result.tools.push(tool("three"));
            }
            Ok(result)
        }
    }

    #[derive(Clone)]
    struct SlowServer;

    impl ServerHandler for SlowServer {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            std::future::pending().await
        }
    }

    fn tool(name: &str) -> Tool {
        Tool::new(
            name.to_owned(),
            "fixture",
            serde_json::Map::from_iter([("type".into(), serde_json::json!("object"))]),
        )
    }

    async fn test_connector<H>(handler: H) -> (GenerationConnector, tokio::task::JoinHandle<()>)
    where
        H: ServerHandler + Send + 'static,
    {
        let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
        let server = tokio::spawn(async move {
            handler
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let generation = McpGeneration::from_test_transport(client_transport, false, None).await;
        let generation = Arc::new(Mutex::new(Some(generation)));
        let connector = GenerationConnector::from_fn(move |_, _, _| {
            let generation = generation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            Box::pin(async move {
                generation.ok_or_else(|| {
                    Box::<dyn std::error::Error + Send + Sync>::from(
                        "scripted generation exhausted",
                    )
                })
            })
        });
        (connector, server)
    }

    fn test_config(server_name: &str) -> Config {
        Config::StreamableHttp {
            server_name: server_name.into(),
            url: "https://example.invalid".into(),
            headers: Default::default(),
            tool_call_timeout_ms: 100,
            startup_timeout_ms: None,
            close_timeout_ms: 100,
            expand_templates: false,
            fail_on_startup_error: false,
            max_instruction_bytes: None,
            reconnect: None,
        }
    }

    #[tokio::test]
    async fn abort_during_connect_budget_has_stable_probe_message() {
        let controller = AbortController::new();
        let signal = controller.signal();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            controller.abort_with("provider-specific cancellation detail");
        });
        let result = bounded(&signal, 10_000, Instant::now(), async {
            std::future::pending::<()>().await
        })
        .await;
        assert_eq!(result, Err("probe aborted"));
    }

    #[test]
    fn provider_error_after_abort_is_normalized() {
        let controller = AbortController::new();
        let signal = controller.signal();
        controller.abort_with("provider detail");
        let binding = McpTemplateBinding::new(
            None,
            Config::StreamableHttp {
                server_name: "docs".into(),
                url: "https://example.invalid".into(),
                headers: Default::default(),
                tool_call_timeout_ms: 10,
                startup_timeout_ms: None,
                close_timeout_ms: 10,
                expand_templates: false,
                fail_on_startup_error: false,
                max_instruction_bytes: None,
                reconnect: None,
            },
        )
        .unwrap();
        assert_eq!(
            probe_connect_error(&signal, &binding, &std::io::Error::other("SECRET")),
            "probe aborted"
        );
    }

    #[tokio::test]
    async fn probe_paginates_and_applies_tool_name_limit() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (connector, server) = test_connector(PagingServer {
            calls: calls.clone(),
        })
        .await;
        let result = probe_server_with_connector(
            None,
            test_config("probe-names"),
            McpProbeOptions {
                timeout_ms: 1_000,
                max_pages: 4,
                max_tool_names: 2,
                signal: None,
            },
            connector,
        )
        .await
        .unwrap();
        assert!(result.close_confirmed);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(matches!(
            result.outcome,
            McpProbeOutcome::Success {
                tool_names,
                truncated: true,
                ..
            } if tool_names == ["one", "two"]
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn probe_applies_page_limit_before_following_cursor() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (connector, server) = test_connector(PagingServer {
            calls: calls.clone(),
        })
        .await;
        let result = probe_server_with_connector(
            None,
            test_config("probe-pages"),
            McpProbeOptions {
                timeout_ms: 1_000,
                max_pages: 1,
                max_tool_names: 10,
                signal: None,
            },
            connector,
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            result.outcome,
            McpProbeOutcome::Success {
                tool_names,
                truncated: true,
                ..
            } if tool_names == ["one"]
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn probe_list_timeout_is_bounded_and_still_closes_generation() {
        let (connector, server) = test_connector(SlowServer).await;
        let result = probe_server_with_connector(
            None,
            test_config("probe-timeout"),
            McpProbeOptions {
                timeout_ms: 20,
                max_pages: 1,
                max_tool_names: 1,
                signal: None,
            },
            connector,
        )
        .await
        .unwrap();
        assert!(result.close_confirmed);
        assert!(matches!(
            result.outcome,
            McpProbeOutcome::Failure {
                stage: McpProbeStage::List,
                ref message,
                ..
            } if message.contains("timed out")
        ));
        server.await.unwrap();
    }
}
