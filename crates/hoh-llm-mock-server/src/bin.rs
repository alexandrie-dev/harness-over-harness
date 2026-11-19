use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use hoh_llm_mock_server::cli::{
    MOCK_LLM_CLI_USAGE, MockLlmCliParseResult, parse_mock_llm_cli_args,
};
use hoh_llm_mock_server::start_mock_llm_server;
use serde_json::json;

fn write_stdout(value: &str) {
    let _ = io::stdout().lock().write_all(value.as_bytes());
}

fn write_json_line(value: &serde_json::Value) {
    write_stdout(&format!("{}\n", hoh_util_values::js_json_stringify(value)));
}

async fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match parse_mock_llm_cli_args(&argv) {
        Ok(value) => value,
        Err(error) => {
            let _ = write!(io::stderr().lock(), "{error}\n\n{MOCK_LLM_CLI_USAGE}");
            return Ok(ExitCode::FAILURE);
        }
    };
    let MockLlmCliParseResult::Run(mut config) = parsed else {
        write_stdout(MOCK_LLM_CLI_USAGE);
        return Ok(ExitCode::SUCCESS);
    };
    let host = config.server.host.as_deref().unwrap_or("127.0.0.1");
    let port = config.server.port.unwrap_or(8_000.0);
    if config.starts_unavailable {
        write_json_line(&json!({
            "type":"unavailable",
            "baseURL":format!("http://{host}:{}/v1", hoh_util_values::js_number_string(port)),
            "listenDelayMs":config.listen_delay_ms,
        }));
        tokio::time::sleep(Duration::from_millis(config.listen_delay_ms)).await;
    }
    config.server.on_event = Some(Arc::new(|event| {
        if let Ok(value) = serde_json::to_value(&event) {
            write_json_line(&value);
        }
    }));
    let mut server = match start_mock_llm_server(config.server).await {
        Ok(server) => server,
        Err(error) => {
            let _ = write!(io::stderr().lock(), "{error}\n\n{MOCK_LLM_CLI_USAGE}");
            return Ok(ExitCode::FAILURE);
        }
    };
    write_json_line(&json!({
        "type":"ready", "baseURL":format!("{}/v1", server.base_url),
        "randomSeed":server.random_seed,
    }));

    #[cfg(unix)]
    let code = {
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            _ = interrupt.recv() => 130,
            _ = terminate.recv() => 143,
        }
    };
    #[cfg(not(unix))]
    let code = {
        tokio::signal::ctrl_c().await?;
        130
    };
    server.close().await;
    Ok(ExitCode::from(code))
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}
