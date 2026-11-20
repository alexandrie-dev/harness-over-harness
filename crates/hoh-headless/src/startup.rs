//! Command-line provider for the one-shot runner.

use crate::json_stream::{JsonSink, bound_json_event_line};
use futures_util::future::BoxFuture;
use hoh_commander::{Command, CommanderError, InvalidArgumentError, Opt, Output};
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Plugin, Service, ServiceKey};
use hoh_loader::cmdline::{CmdlineArgsKey, parse_cmdline};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{IsTerminal, Read, Write},
    path::Path,
    sync::{Arc, Mutex},
};

pub const HEADLESS_STARTUP_SERVICE: &str = "headlessStartup";
const TASK_REQUIRED: &str =
    "error: a task is required, for example: dsh --profile headless \"run the tests\"";

/// Arguments accepted by the headless runner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadlessStartup {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub json: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_writable_roots: Option<Vec<String>>,
}

pub struct HeadlessStartupKey;
impl ServiceKey for HeadlessStartupKey {
    type Value = HeadlessStartup;
    const NAME: &'static str = HEADLESS_STARTUP_SERVICE;

    fn view(value: &Self::Value) -> Option<Value> {
        serde_json::to_value(value).ok()
    }
}

/// Host seam for stdio and terminal detection. Tests can capture it without process-global swaps.
pub struct HeadlessRuntime {
    pub output: Output,
    pub stdout: Arc<dyn JsonSink>,
    pub stderr: Arc<dyn JsonSink>,
    pub stdin_is_tty: Arc<dyn Fn() -> bool + Send + Sync>,
    pub read_stdin: Arc<dyn Fn() -> BoxFuture<'static, std::io::Result<String>> + Send + Sync>,
}

impl Clone for HeadlessRuntime {
    fn clone(&self) -> Self {
        Self {
            output: self.output.clone(),
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            stdin_is_tty: self.stdin_is_tty.clone(),
            read_stdin: self.read_stdin.clone(),
        }
    }
}

impl Default for HeadlessRuntime {
    fn default() -> Self {
        let stdout: Arc<dyn JsonSink> = Arc::new(|chunk: &str| {
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(chunk.as_bytes());
            let _ = stdout.flush();
        });
        let stderr: Arc<dyn JsonSink> = Arc::new(|chunk: &str| {
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(chunk.as_bytes());
            let _ = stderr.flush();
        });
        Self {
            output: Output::default(),
            stdout,
            stderr,
            stdin_is_tty: Arc::new(|| std::io::stdin().is_terminal()),
            read_stdin: Arc::new(|| {
                Box::pin(async {
                    tokio::task::spawn_blocking(|| {
                        let mut text = String::new();
                        std::io::stdin().read_to_string(&mut text)?;
                        Ok(text)
                    })
                    .await
                    .map_err(std::io::Error::other)?
                })
            }),
        }
    }
}

hoh_kernel::service!(pub HeadlessRuntimeKey: HeadlessRuntime = "headlessRuntimeHooks");

fn collect(value: &str, previous: Option<&Value>) -> Result<Value, InvalidArgumentError> {
    let mut values = previous
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    values.push(Value::String(value.into()));
    Ok(Value::Array(values))
}

/// The exact source command declaration.
pub fn headless_command() -> Command {
    let mut program = Command::new();
    program
        .name("dsh --profile headless")
        .description(
            "Answer one task and exit; the answer goes to stdout and diagnostics to stderr.",
        )
        .help_option("-h, --help", "show this help")
        .option(
            "--json",
            "write newline-delimited run events to stdout instead of the final message",
        )
        .option(
            "--session-id <id>",
            "adopt the persisted Session with this id; an unknown id is an error",
        )
        .option(
            "--permission-mode <preset>",
            "select a configured permission preset for this task, including on resume",
        )
        .add_option(
            Opt::new(
                "--approval-policy <policy>",
                "set approval behavior after selecting the permission preset",
            )
            .choices(["ask", "never"]),
        )
        .option_with_parser(
            "--add-dir <path>",
            "allow an additional physical absolute directory under workspace-write (repeatable)",
            collect,
        )
        .argument(
            "[task...]",
            "the task text; multiple words are joined by spaces, and `-` reads stdin",
        )
        .add_help_text(
            "after",
            "\nExamples:\n  dsh --profile headless \"run the tests\"          answer one task and exit\n  echo \"run the tests\" | dsh --profile headless   read the task from stdin\n  dsh --profile headless --json \"run the tests\"   emit machine-readable run events\n  dsh --profile headless --session-id session-… \"continue\"   resume an existing Session\n  dsh --profile headless --permission-mode workspace-write --add-dir /repo/.git \"commit the fix\"\n                                                        grant shared Git metadata for this task\n",
        );
    program
}

/// Scan the raw invocation without treating option operands or post-`--` text as flags.
pub fn json_requested(argv: &[String]) -> bool {
    let mut index = 0;
    while index < argv.len() {
        match argv[index].as_str() {
            "--" => return false,
            "--json" => return true,
            "--session-id" | "--permission-mode" | "--approval-policy" | "--add-dir" => {
                index += 1;
            }
            _ => {}
        }
        index += 1;
    }
    false
}

#[derive(Inject)]
pub struct Deps {
    cmdline_args: Service<CmdlineArgsKey>,
}

pub struct HeadlessStartupPlugin;

impl Plugin for HeadlessStartupPlugin {
    const NAME: &'static str = "headless-startup";
    type Config = NoConfig;
    type Deps = Deps;

    async fn apply(
        ctx: Context,
        deps: Deps,
        _config: Cfg<NoConfig>,
    ) -> Result<(), hoh_kernel::Error> {
        let runtime = ctx
            .get::<HeadlessRuntimeKey>()
            .map(|runtime| (*runtime).clone())
            .unwrap_or_default();
        let accepted = Arc::new(Mutex::new(None::<HeadlessStartup>));
        let slot = accepted.clone();
        let tty = runtime.stdin_is_tty.clone();
        let mut program = headless_command();
        if json_requested(deps.cmdline_args.get()) {
            let stdout = runtime.stdout.clone();
            program.error_handler(move |message, exit_code, code| {
                let message = message.strip_prefix("error: ").unwrap_or(message);
                stdout.write(&format!(
                    "{}\n",
                    bound_json_event_line(&json!({"type":"error", "message":message}))
                ));
                CommanderError::new(exit_code, code, message)
            });
        }
        program.action(move |_, options, command| {
            if command.args.len() > 1 && command.args.iter().any(|value| value == "-") {
                return Err(command.error("error: `-` must be the only task argument").into());
            }
            let joined = command.args.join(" ");
            if !command.args.is_empty() && joined.trim().is_empty() {
                return Err(command.error(TASK_REQUIRED).into());
            }
            let task = (!command.args.is_empty()).then_some(joined);
            if task.is_none() && tty() {
                return Err(command.error(TASK_REQUIRED).into());
            }
            let session_id = options
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if session_id.as_deref().is_some_and(|value| value.trim().is_empty()) {
                return Err(command
                    .error("error: --session-id requires a non-empty session id")
                    .into());
            }
            let permission_mode = options
                .get("permissionMode")
                .and_then(Value::as_str)
                .map(str::trim)
                .map(str::to_owned);
            if permission_mode.as_deref() == Some("") {
                return Err(command
                    .error("error: --permission-mode requires a non-empty preset name")
                    .into());
            }
            let roots = options.get("addDir").and_then(Value::as_array).map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            });
            if roots.as_ref().is_some_and(|roots| {
                roots
                    .iter()
                    .any(|root| !Path::new(root).is_absolute() || root.contains('\0'))
            }) {
                return Err(command
                    .error(
                        "error: --add-dir requires an absolute directory path in the execution environment",
                    )
                    .into());
            }
            let mut deduplicated = Vec::new();
            if let Some(roots) = roots {
                for root in roots {
                    if !deduplicated.contains(&root) {
                        deduplicated.push(root);
                    }
                }
            }
            *slot.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(HeadlessStartup {
                task,
                session_id,
                json: options.get("json") == Some(&Value::Bool(true)),
                permission_mode,
                approval_policy: options
                    .get("approvalPolicy")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                additional_writable_roots: (!deduplicated.is_empty()).then_some(deduplicated),
            });
            Ok(())
        });
        parse_cmdline(&ctx, &mut program, &runtime.output)?;
        let startup = accepted
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        if let Some(startup) = startup {
            ctx.provide::<HeadlessStartupKey>(Arc::new(startup)).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_scan_skips_values_and_stops_at_separator() {
        assert!(!json_requested(&["--session-id".into(), "--json".into()]));
        assert!(!json_requested(&[
            "--permission-mode".into(),
            "--json".into()
        ]));
        assert!(!json_requested(&[
            "--approval-policy".into(),
            "--json".into()
        ]));
        assert!(!json_requested(&["--add-dir".into(), "--json".into()]));
        assert!(!json_requested(&["--".into(), "--json".into()]));
        assert!(json_requested(&["--json".into(), "task".into()]));
    }

    #[test]
    fn command_declares_repeatable_roots_and_task_positionals() {
        let mut command = headless_command();
        command.exit_override();
        command
            .parse(
                ["--add-dir", "/first", "--add-dir", "/second", "one", "two"],
                hoh_commander::ParseOptions::USER,
            )
            .unwrap();
        assert_eq!(command.args, ["one", "two"]);
        assert_eq!(command.opts()["addDir"], json!(["/first", "/second"]));
    }
}
