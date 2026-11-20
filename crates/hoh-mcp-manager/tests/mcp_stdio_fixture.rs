use std::{io::Write, time::Duration};

use rmcp::{
    ServerHandler, ServiceExt,
    model::{
        ErrorData, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::{RequestContext, RoleServer},
};
use serde_json::json;

const HELPER_ENV: &str = "HOH_MCP_MANAGER_STDIO_HELPER";
const HELPER_TOOL_ENV: &str = "HOH_MCP_MANAGER_STDIO_TOOL";
const HELPER_START_LOG_ENV: &str = "HOH_MCP_MANAGER_STDIO_START_LOG";
const HELPER_DELAY_MS_ENV: &str = "HOH_MCP_MANAGER_STDIO_DELAY_MS";
const HELPER_HANG_AFTER_FIRST_ENV: &str = "HOH_MCP_MANAGER_STDIO_HANG_AFTER_FIRST";

#[derive(Clone)]
struct FixtureServer {
    tool_name: String,
}

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut result = ListToolsResult::default();
        result.tools.push(Tool::new(
            self.tool_name.clone(),
            "manager live-transport fixture",
            serde_json::Map::from_iter([("type".into(), json!("object"))]),
        ));
        Ok(result)
    }
}

#[tokio::main]
async fn main() {
    if std::env::var(HELPER_ENV).as_deref() != Ok("1") {
        return;
    }
    let mut start_number = 1;
    if let Ok(path) = std::env::var(HELPER_START_LOG_ENV) {
        start_number = std::fs::read_to_string(&path)
            .map(|text| text.lines().count() + 1)
            .unwrap_or(1);
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        log.write_all(b"1\n").unwrap();
        log.flush().unwrap();
    }
    if let Ok(delay) = std::env::var(HELPER_DELAY_MS_ENV) {
        tokio::time::sleep(Duration::from_millis(delay.parse().unwrap())).await;
    }
    let tool_name = std::env::var(HELPER_TOOL_ENV).unwrap_or_else(|_| "echo".into());
    let hang_list_tools =
        std::env::var(HELPER_HANG_AFTER_FIRST_ENV).as_deref() == Ok("1") && start_number > 1;
    if hang_list_tools {
        // A server that never answers `initialize` but leaves when its stdin closes, so the
        // manager's cleanup of an aborted probe can be confirmed.
        let mut sink = String::new();
        while std::io::stdin()
            .read_line(&mut sink)
            .is_ok_and(|read| read > 0)
        {
            sink.clear();
        }
        return;
    }
    FixtureServer { tool_name }
        .serve(rmcp::transport::stdio())
        .await
        .unwrap()
        .waiting()
        .await
        .unwrap();
}
