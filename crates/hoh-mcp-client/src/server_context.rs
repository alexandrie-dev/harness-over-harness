use crate::{
    diagnostic::{
        McpLocalError, McpLocalErrorCode, McpToolError, McpToolErrorCode, mcp_request_error,
    },
    generation::McpGeneration,
    tools::McpRedact,
};
use hoh_error::BoxError;
use hoh_mcp_resources::{McpResourceFuture, McpResourceProvider, McpResourceRequest};
use hoh_tools::ToolExecution;
use rmcp::model::{PaginatedRequestParams, ReadResourceRequestParams};
use std::sync::Arc;

#[derive(Clone)]
pub struct GenerationResourceProvider {
    generation: McpGeneration,
    timeout_ms: u64,
    redact: Option<McpRedact>,
}
impl GenerationResourceProvider {
    pub fn new(generation: McpGeneration) -> Self {
        Self {
            generation,
            timeout_ms: crate::DEFAULT_TOOL_CALL_TIMEOUT_MS,
            redact: None,
        }
    }

    pub fn with_policy(
        generation: McpGeneration,
        timeout_ms: u64,
        redact: Option<McpRedact>,
    ) -> Self {
        Self {
            generation,
            timeout_ms,
            redact,
        }
    }
}

impl McpResourceProvider for GenerationResourceProvider {
    fn request<'a>(
        &'a self,
        request: McpResourceRequest,
        exec: Arc<ToolExecution>,
    ) -> McpResourceFuture<'a> {
        Box::pin(async move {
            let operation = request.method();
            let has_resources = self.generation.has_resources().await;
            let operation_future = async {
                match request {
                    McpResourceRequest::List { .. } if !has_resources => {
                        Ok(serde_json::json!({ "resources": [] }))
                    }
                    McpResourceRequest::ListTemplates { .. } if !has_resources => {
                        Ok(serde_json::json!({ "resourceTemplates": [] }))
                    }
                    McpResourceRequest::List { cursor } => self
                        .generation
                        .list_resources(cursor.map(|cursor| {
                            PaginatedRequestParams::default().with_cursor(Some(cursor))
                        }))
                        .await
                        .and_then(json_value),
                    McpResourceRequest::ListTemplates { cursor } => self
                        .generation
                        .list_resource_templates(cursor.map(|cursor| {
                            PaginatedRequestParams::default().with_cursor(Some(cursor))
                        }))
                        .await
                        .and_then(json_value),
                    McpResourceRequest::Read { uri } => self
                        .generation
                        .read_resource(ReadResourceRequestParams::new(uri))
                        .await
                        .and_then(json_value),
                }
            };
            let signal = exec.signal();
            signal
                .throw_if_aborted()
                .map_err(|error| Box::new(error) as BoxError)?;
            let result = tokio::select! {
                reason = signal.cancelled() => return Err(Box::new(reason) as BoxError),
                result = tokio::time::timeout(
                    std::time::Duration::from_millis(self.timeout_ms.max(1)),
                    operation_future,
                ) => result,
            };
            let result = result.map_err(|_| {
                Box::new(McpToolError::new(
                    format!(
                        "MCP {operation} failed: request timeout after {}ms",
                        self.timeout_ms
                    ),
                    McpToolErrorCode::Timeout,
                )) as BoxError
            })?;
            result.map_err(|error| {
                Box::new(mcp_request_error(error, operation, self.redact.as_deref())) as BoxError
            })
        })
    }
}

fn json_value(value: impl serde::Serialize) -> Result<serde_json::Value, BoxError> {
    serde_json::to_value(value).map_err(|error| Box::new(error) as BoxError)
}

pub fn instruction_text(
    server: &str,
    instructions: &str,
    max_bytes: usize,
) -> Result<String, BoxError> {
    let instructions = instructions.trim_end();
    let attributed = if instructions.is_empty() {
        String::new()
    } else {
        format!("### MCP server: {server}\n\n{instructions}")
    };
    if attributed.len() > max_bytes {
        return Err(Box::new(McpLocalError::new(
            format!(
                "mcp-client({server}): server instructions exceed maxInstructionBytes ({max_bytes})"
            ),
            McpLocalErrorCode::InstructionsTooLarge,
        )));
    }
    Ok(attributed)
}
