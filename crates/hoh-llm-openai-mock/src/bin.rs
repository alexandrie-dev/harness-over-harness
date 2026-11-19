use std::io::{self, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use hoh_llm_openai_mock::{MockOptions, MockServer, Scenario};
use serde_json::json;

const USAGE: &str =
    "Usage: hoh-llm-openai-mock --scenario FILE [--journal FILE] [--listen HOST:PORT]\n";

struct Args {
    scenario: PathBuf,
    journal: Option<PathBuf>,
    host: String,
    port: u16,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut args = args.into_iter();
    let mut scenario = None;
    let mut journal = None;
    let mut host = "127.0.0.1".to_owned();
    let mut port = 0;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--scenario" => {
                scenario = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--scenario needs a file".to_owned())?,
                ));
            }
            "--journal" => {
                journal = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--journal needs a file".to_owned())?,
                ));
            }
            "--listen" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--listen needs HOST:PORT".to_owned())?;
                let (parsed_host, parsed_port) = value
                    .rsplit_once(':')
                    .ok_or_else(|| "--listen must be HOST:PORT".to_owned())?;
                if parsed_host.is_empty() {
                    return Err("--listen host must not be empty".to_owned());
                }
                host = parsed_host.trim_matches(['[', ']']).to_owned();
                port = parsed_port
                    .parse()
                    .map_err(|_| "--listen port must be 0..65535".to_owned())?;
            }
            "--help" | "-h" => return Err(String::new()),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(Args {
        scenario: scenario.ok_or_else(|| "--scenario is required".to_owned())?,
        journal,
        host,
        port,
    })
}

async fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            if !error.is_empty() {
                let _ = writeln!(io::stderr().lock(), "{error}");
            }
            let _ = write!(io::stderr().lock(), "{USAGE}");
            return Ok(if error.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
    };
    let scenario = Scenario::from_path(&args.scenario)?;
    let mut options = MockOptions::new(scenario);
    options.host = args.host;
    options.port = args.port;
    options.journal_path = args.journal;
    // Handlers first: a signal that follows the `ready` line must already find them.
    #[cfg(unix)]
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut server = MockServer::start(options).await?;
    println!(
        "{}",
        json!({"type":"ready","baseURL":server.base_url,"port":server.port})
    );
    io::stdout().flush()?;

    #[cfg(unix)]
    let code = tokio::select! {
        _ = interrupt.recv() => 130,
        _ = terminate.recv() => 143,
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

#[cfg(test)]
mod tests {
    use super::parse_args;

    #[test]
    fn parses_required_and_optional_arguments() {
        let args = parse_args(
            [
                "--scenario",
                "scenario.json",
                "--journal",
                "journal.jsonl",
                "--listen",
                "127.0.0.1:8123",
            ]
            .map(str::to_owned),
        )
        .expect("arguments");
        assert_eq!(args.host, "127.0.0.1");
        assert_eq!(args.port, 8123);
        assert_eq!(args.scenario.to_string_lossy(), "scenario.json");
        assert_eq!(
            args.journal.expect("journal").to_string_lossy(),
            "journal.jsonl"
        );
    }

    fn error_of(arguments: &[&str]) -> String {
        match parse_args(arguments.iter().map(|argument| (*argument).to_owned())) {
            Ok(_) => panic!("{arguments:?} must be refused"),
            Err(error) => error,
        }
    }

    #[test]
    fn refuses_bad_arguments_with_a_reason() {
        assert_eq!(error_of(&[]), "--scenario is required");
        assert_eq!(error_of(&["--scenario"]), "--scenario needs a file");
        assert_eq!(
            error_of(&["--scenario", "s", "--journal"]),
            "--journal needs a file"
        );
        assert_eq!(
            error_of(&["--scenario", "s", "--listen"]),
            "--listen needs HOST:PORT"
        );
        assert_eq!(
            error_of(&["--scenario", "s", "--listen", "8123"]),
            "--listen must be HOST:PORT"
        );
        assert_eq!(
            error_of(&["--scenario", "s", "--listen", ":8123"]),
            "--listen host must not be empty"
        );
        assert_eq!(
            error_of(&["--scenario", "s", "--listen", "127.0.0.1:70000"]),
            "--listen port must be 0..65535"
        );
        assert_eq!(error_of(&["--bogus"]), "unknown argument: --bogus");
        assert_eq!(error_of(&["--help"]), "");
    }

    #[test]
    fn accepts_a_bracketed_ipv6_listen_address() {
        let args = parse_args(["--scenario", "s", "--listen", "[::1]:9"].map(str::to_owned))
            .expect("arguments");
        assert_eq!((args.host.as_str(), args.port), ("::1", 9));
    }
}
