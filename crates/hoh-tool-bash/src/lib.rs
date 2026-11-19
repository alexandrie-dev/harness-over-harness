//! Port of @deepseek-ai/dsh-tool-bash (packages/shell/tool-bash).
//!
//! Model-facing consumer of the `shell` capability seam. While a job registry is composed, every
//! call registers its process with `jobs` as it starts: `run_in_background` returns the id at once,
//! and a foreground call waits on its job until the command finishes or the wait times out, at which
//! point it returns the same id. Without a registry the tool is foreground-only and the executor's
//! deadline kills the command.

pub mod background;
mod execute;
pub mod render;

use std::sync::{Arc, Mutex, MutexGuard};

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, FiberState, Inject, Plugin, Service, Undo};
use hoh_llm::ContentBlock;
use hoh_sandbox::{ESCALATION_TARGETS, SandboxMode, sandbox_permissions_description};
use hoh_shell::{DSH_ENV_PREFIX, Shell};
use hoh_shell_env::ShellEnv;
use hoh_system_prompt::{PromptSection, PromptSectionOrderName, SystemPrompt, SystemPromptKey};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, GenericCallView, TerminalCallView, ToolCallKind,
    ToolCallView, ToolDefinition, ToolRegistrationOptions, ToolResult, ToolResultView, Tools,
    define_tool,
};
use hoh_working_directory::WorkingDirectoryKey;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use hoh_jobs::Jobs;
use hoh_sandbox_policy::SandboxPolicy;

/// Mount dependencies of the plugin (`inject` of the source).
#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
    shell: Service<Shell>,
    system_prompt: Service<SystemPromptKey>,
    shell_env: Service<ShellEnv>,
    working_directory: Service<WorkingDirectoryKey>,
}

fn default_true() -> bool {
    true
}

/// Configuration for the bash tool.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
#[schemars(extend("default" = serde_json::json!({})))]
pub struct Config {
    /// Expose `run_in_background` while a job registry is composed (default true); disabled calls
    /// are also rejected. Without a registry the tool is foreground-only regardless.
    #[serde(default = "default_true")]
    pub enable_run_in_background: bool,
    /// Keep a foreground command that reaches its timeout running as a background job instead of
    /// killing it (default true). Applies only while background execution is available: with
    /// `enableRunInBackground` false or no job registry, the executor's deadline kills the command.
    /// A foreground command the registry refuses at its start (admission or a missing controller)
    /// also runs under the deadline kill.
    #[serde(default = "default_true")]
    pub promote_on_timeout: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enable_run_in_background: true,
            promote_on_timeout: true,
        }
    }
}

/// What every registration of the tool shares: the mounted services and the composition facts that
/// are fixed at load.
pub(crate) struct Shared {
    pub ctx: Context,
    pub tools: Service<Tools>,
    pub shell: Service<Shell>,
    pub shell_env: Service<ShellEnv>,
    pub working_directory: Service<WorkingDirectoryKey>,
    pub background_enabled: bool,
    pub promote_on_timeout: bool,
    pub escalation_modes: Vec<SandboxMode>,
    pub sandbox_policy: Option<Service<SandboxPolicy>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `bashDescription()`: the model-facing description of the tool.
fn bash_description() -> String {
    format!(
        "Execute a bash command (`bash -c`) and return its stdout/stderr. \
         Each call runs in a fresh shell; pass `workdir` instead of using `cd`. \
         Managed `${DSH_ENV_PREFIX}*` variables expose current harness environment facts. \
         Long output is truncated to its tail; the full output is saved to a file whose path is reported when available. \
         Provide `description` before `command` in the arguments. \
         Before any delete or move, verify that the resolved absolute target path is the intended one; never run it against a computed path you have not checked. \
         An unset variable expands to an empty string, so guard variables in such paths with `${{VAR:?}}`. \
         Commands may run under a file sandbox; a blocked file operation is reported as `[sandbox: file access denied under <mode> mode]`, a policy denial: do not retry another way."
    )
}

/// The parameter schema the registered tool advertises in this composition.
fn parameters(background: bool, promote: bool, escalation_modes: &[SandboxMode]) -> Value {
    let mut parameters = Map::new();
    parameters.insert(
        "description".into(),
        json!({
            "type": "string",
            "required": true,
            "description": "Clear, concise description of what this command does in active voice, 5-10 words (shown in the UI). Examples: \"ls\" → \"List files in current directory\"; \"git status\" → \"Show working tree status\"; \"npm install\" → \"Install package dependencies\".",
        }),
    );
    parameters.insert(
        "command".into(),
        json!({ "type": "string", "required": true, "description": "The bash command to execute." }),
    );
    parameters.insert(
        "timeoutMs".into(),
        json!({
            "type": "number",
            "description": if promote {
                "Timeout in milliseconds. The executor applies its configured default and cap; on expiry the command moves to the background as a job instead of being killed."
            } else {
                "Timeout in milliseconds. The executor applies its configured default and cap, and kills the command on expiry."
            },
        }),
    );
    parameters.insert(
        "workdir".into(),
        json!({ "type": "string", "description": "Working directory for this command. Defaults to the session workspace; a relative path is resolved against it." }),
    );
    if background {
        parameters.insert(
            "run_in_background".into(),
            json!({ "type": "boolean", "description": "Run in the background and return a job id immediately (collect with job_output, stop with job_kill). No timeout applies." }),
        );
    }
    if !escalation_modes.is_empty() {
        parameters.insert(
            "sandbox_permissions".into(),
            json!({
                "type": "string",
                "enum": escalation_modes.iter().map(|mode| mode.as_str()).collect::<Vec<_>>(),
                "description": sandbox_permissions_description("command"),
            }),
        );
        parameters.insert(
            "justification".into(),
            json!({
                "type": "string",
                "description": "Required with sandbox_permissions: one sentence for the user explaining why this exact command needs the wider access. Use the language of the user’s current request.",
            }),
        );
    }
    Value::Object(parameters)
}

/// The canonical output union: background handle, promoted job, foreground result.
fn output_schema() -> Value {
    let stream = || {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": true,
            "properties": {
                "text": { "type": "string", "required": true },
                "truncated": { "type": "boolean", "required": true },
                "spillPath": { "type": "string" },
            },
        })
    };
    json!({
        "oneOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": { "type": "string", "required": true, "const": "background" },
                    "jobId": { "type": "string", "required": true },
                    "cwd": { "type": "string", "required": true },
                },
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": { "type": "string", "required": true, "const": "promoted" },
                    "cwd": { "type": "string", "required": true },
                    "jobId": { "type": "string", "required": true },
                    "timeoutMs": { "type": "number", "required": true },
                    "output": { "type": "string", "required": true },
                },
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": { "type": "string", "required": true, "const": "foreground" },
                    "cwd": { "type": "string", "required": true },
                    "exitCode": { "required": true, "oneOf": [{ "type": "integer" }, { "type": "null" }] },
                    "signal": { "required": true, "oneOf": [{ "type": "string" }, { "type": "null" }] },
                    "timedOut": { "type": "boolean", "required": true },
                    "aborted": { "type": "boolean", "required": true },
                    "stopped": { "type": "string" },
                    "timeoutMs": { "type": "number", "required": true },
                    "stdout": stream(),
                    "stderr": stream(),
                    "sandbox": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "mode": { "type": "string", "required": true },
                            "denied": { "type": "boolean", "required": true },
                            "enforcement": { "type": "string" },
                            "runnerFailed": { "type": "boolean" },
                        },
                    },
                },
            },
        ],
    })
}

/// Present foreground calls as terminals and background starts as generic cards. The command remains
/// the title on both paths; foreground cwd is passed through for the bridge to resolve, while
/// background descriptions remain card content.
fn present_bash_call(args: &Value) -> Option<ToolCallView> {
    let command = args.get("command")?.as_str()?;
    let description = args.get("description")?.as_str()?;
    if args.get("run_in_background") == Some(&Value::Bool(true)) {
        return Some(ToolCallView::Generic(GenericCallView {
            title: command.to_owned(),
            kind: Some(ToolCallKind::Execute),
            raw_input: Some(Value::String(command.to_owned())),
            content: Some(vec![ContentBlock::text(description)]),
            locations: None,
        }));
    }
    Some(ToolCallView::Terminal(TerminalCallView {
        title: command.to_owned(),
        description: Some(description.to_owned()),
        cwd: args
            .get("workdir")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }))
}

/// Present completed foreground output as a terminal; background acknowledgements and execution
/// errors use generic fenced output without an exit-status pill.
fn present_bash_result(args: &Value, result: &ToolResult) -> Option<ToolResultView> {
    let [block] = result.content.as_slice() else {
        return None;
    };
    if block.r#type != "text" {
        return None;
    }
    let raw = block.text_value().unwrap_or_default();
    let is_background = args.get("run_in_background") == Some(&Value::Bool(true));
    // The source reads `result.value?.kind === 'promoted'`, but a `ToolResult` has no `value`
    // member, so the test never holds (PORT_NOTES).
    let is_promoted = false;
    // Background acknowledgements, promotions, and errors have no terminal exit status.
    if is_background || is_promoted || result.is_error {
        let trimmed = raw.trim_end_matches('\n');
        return Some(ToolResultView::Generic(hoh_tools::GenericResultView {
            title: None,
            content: Some(vec![ContentBlock::text(format!(
                "```console\n{trimmed}\n```"
            ))]),
        }));
    }
    // The exit marker becomes the card's exit pill, so it leaves the output body.
    Some(match hoh_shell::parse_exit_status(raw) {
        hoh_shell::ParsedExitStatus::ExitCode { body, exit_code } => {
            ToolResultView::Terminal(hoh_tools::TerminalResultView {
                title: None,
                output: Some(body),
                exit_code: i64::try_from(exit_code).ok(),
                signal: None,
            })
        }
        hoh_shell::ParsedExitStatus::Signal { body, signal } => {
            ToolResultView::Terminal(hoh_tools::TerminalResultView {
                title: None,
                output: Some(body),
                exit_code: None,
                signal: Some(signal),
            })
        }
    })
}

/// One registration of the `bash` tool. With a registry, every call registers its process as a job at
/// its start; without one the tool is foreground-only and the executor's deadline kills the command.
fn bash_tool(
    shared: &Arc<Shared>,
    jobs: Option<Service<Jobs>>,
) -> Result<ToolDefinition, BoxError> {
    let background = jobs.is_some();
    let promote = background && shared.promote_on_timeout;
    let escalation_modes = shared.escalation_modes.clone();
    let render_modes = escalation_modes.clone();
    let execute_shared = shared.clone();
    let mut options = DefineToolOptions::new(
        "bash",
        bash_description(),
        parameters(background, promote, &escalation_modes),
        DefineToolOutput {
            schema: output_schema(),
            render: Arc::new(move |_args, value| {
                vec![ContentBlock::text(execute::render_value(
                    value,
                    &render_modes,
                ))]
            }),
            presentation_meta: Some(Arc::new(
                |_args, value| json!({ "cwd": value.get("cwd").cloned().unwrap_or(Value::Null) }),
            )),
        },
        move |args, exec| {
            Box::pin(execute::execute(
                execute_shared.clone(),
                jobs.clone(),
                promote,
                args,
                exec,
            ))
        },
    );
    options.present_call = Some(Arc::new(present_bash_call));
    options.present_result = Some(Arc::new(present_bash_result));
    define_tool(options)
}

fn register_tool(shared: &Arc<Shared>, jobs: Option<Service<Jobs>>) -> Result<Disposer, BoxError> {
    let definition = bash_tool(shared, jobs)?;
    shared
        .tools
        .register(&shared.ctx, definition, ToolRegistrationOptions::default())
}

/// Mount dependency of the job-backed variant.
#[derive(Inject)]
struct JobsDeps {
    jobs: Service<Jobs>,
}

/// The bash tool plugin (`name = 'tool-bash'`).
pub struct ToolBash;

impl Plugin for ToolBash {
    const NAME: &'static str = "tool-bash";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = config.get();
        let background_enabled = config.enable_run_in_background;
        // Keeping a timed-out command needs the whole background surface: the job tools to collect
        // and stop it, and the registry itself at execution time.
        let promote_on_timeout = config.promote_on_timeout && background_enabled;
        let default_mode = deps.shell.sandbox_mode();
        let escalation_modes: Vec<SandboxMode> = if default_mode.is_none() {
            Vec::new()
        } else {
            ESCALATION_TARGETS.to_vec()
        };
        let sandbox_policy = if default_mode.is_none() {
            None
        } else {
            ctx.get::<SandboxPolicy>()
        };
        if default_mode.is_some() && sandbox_policy.is_none() {
            return Err(
                "tool-bash: the mounted bash executor confines but ctx.sandboxPolicy is missing"
                    .into(),
            );
        }

        // Cross-call guidance belongs in the prompt rather than one-call schema prose.
        deps.system_prompt.section(
            &ctx,
            PromptSection {
                name: "tool:bash".to_owned(),
                order: SystemPrompt::get_section_order(PromptSectionOrderName::ToolBash),
                text: "Check the [exit code: N] marker on every bash result; investigate failures before moving on."
                    .into(),
                interpolate: None,
                complete: false,
            },
        )?;

        let shared = Arc::new(Shared {
            ctx: ctx.clone(),
            tools: deps.tools.clone(),
            shell: deps.shell.clone(),
            shell_env: deps.shell_env.clone(),
            working_directory: deps.working_directory.clone(),
            background_enabled,
            promote_on_timeout,
            escalation_modes,
            sandbox_policy,
        });

        // The background surface follows the registry. The foreground-only variant registers now
        // unless a registry is already composed, the job-backed variant replaces it for as long as
        // `jobs` is present, and the foreground-only one returns when the registry unloads while
        // this plugin stays. Both registrations belong to this plugin's own fiber.
        if !background_enabled {
            register_tool(&shared, None)?;
            return Ok(());
        }
        let foreground_only: Arc<Mutex<Option<Disposer>>> =
            Arc::new(Mutex::new(if ctx.get::<Jobs>().is_none() {
                Some(register_tool(&shared, None)?)
            } else {
                None
            }));
        ctx.inject::<JobsDeps, _>(move |job_ctx, deps| {
            let shared = shared.clone();
            let foreground_only = foreground_only.clone();
            async move {
                let previous = lock(&foreground_only).take();
                if let Some(previous) = previous {
                    previous.dispose().await;
                }
                let unregister = register_tool(&shared, Some(deps.jobs.clone()))?;
                job_ctx.effect("tool-bash job-backed registration", move || {
                    Undo::future(move || async move {
                        unregister.dispose().await;
                        if shared.ctx.fiber().state() == FiberState::Active {
                            *lock(&foreground_only) = Some(register_tool(&shared, None)?);
                        }
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hoh_shell::{CollectedOutput, ShellRunResult};
    use serde_json::{Value, json};

    use super::*;
    use crate::render::{RenderedRun, render_result};

    fn text_result(text: &str, is_error: bool) -> ToolResult {
        ToolResult {
            content: vec![ContentBlock::text(text)],
            is_error,
            meta: None,
        }
    }

    fn call(args: Value) -> Value {
        serde_json::to_value(present_bash_call(&args).expect("a view")).unwrap()
    }

    fn present(args: Value, result: &ToolResult) -> Option<Value> {
        present_bash_result(&args, result).map(|view| serde_json::to_value(view).unwrap())
    }

    #[test]
    fn a_foreground_call_is_a_terminal_card_and_workdir_passes_through_as_is() {
        assert_eq!(
            call(json!({"command": "ls -la src", "description": "List files in src"})),
            json!({"card": "terminal", "title": "ls -la src", "description": "List files in src"})
        );
        assert_eq!(
            call(json!({"command": "pwd", "description": "Print dir", "workdir": "/tmp/x"})),
            json!({"card": "terminal", "title": "pwd", "description": "Print dir", "cwd": "/tmp/x"})
        );
        assert_eq!(
            call(json!({"command": "pwd", "description": "Print dir", "workdir": "sub"})),
            json!({"card": "terminal", "title": "pwd", "description": "Print dir", "cwd": "sub"})
        );
    }

    #[test]
    fn a_background_call_is_a_generic_execute_card_and_its_ack_has_no_exit_pill() {
        assert_eq!(
            call(json!({"command": "sleep 100", "description": "wait", "run_in_background": true})),
            json!({
                "card": "generic", "title": "sleep 100", "kind": "execute",
                "rawInput": "sleep 100", "content": [{"type": "text", "text": "wait"}]
            })
        );
        assert_eq!(
            present(
                json!({"command": "sleep 100", "description": "wait", "run_in_background": true}),
                &text_result("started background job bash-1", false)
            ),
            Some(json!({
                "card": "generic",
                "content": [{"type": "text", "text": "```console\nstarted background job bash-1\n```"}]
            }))
        );
    }

    #[test]
    fn a_terminal_result_keeps_raw_output_and_parses_the_exit() {
        let args = json!({"command": "x", "description": "x"});
        assert_eq!(
            present(args.clone(), &text_result("hi\n\n", false)),
            Some(json!({"card": "terminal", "output": "hi\n\n", "exitCode": 0}))
        );
        assert_eq!(
            present(args.clone(), &text_result("oops\n[exit code: 3]", false)),
            Some(json!({"card": "terminal", "output": "oops", "exitCode": 3}))
        );
        assert_eq!(
            present(
                args.clone(),
                &text_result("gone\n[killed by signal: SIGKILL]", false)
            ),
            Some(json!({"card": "terminal", "output": "gone", "signal": "SIGKILL"}))
        );
        // Markers a pill cannot show stay in the output.
        assert_eq!(
            present(
                args.clone(),
                &text_result("slow\n[timed out after 100ms]\n[exit code: 143]", false)
            ),
            Some(json!({
                "card": "terminal", "output": "slow\n[timed out after 100ms]", "exitCode": 143
            }))
        );
        // Marker-like text without the leading newline is output, not a failure.
        assert_eq!(
            present(args.clone(), &text_result("[exit code: 5]", false)),
            Some(json!({"card": "terminal", "output": "[exit code: 5]", "exitCode": 0}))
        );
        assert_eq!(
            present(args, &text_result("[killed by signal: SIGKILL]", false)),
            Some(json!({
                "card": "terminal", "output": "[killed by signal: SIGKILL]", "exitCode": 0
            }))
        );
    }

    #[test]
    fn an_error_result_is_a_generic_card() {
        assert_eq!(
            present(
                json!({"command": "x", "description": "x"}),
                &text_result("tool call aborted", true)
            ),
            Some(json!({
                "card": "generic",
                "content": [{"type": "text", "text": "```console\ntool call aborted\n```"}]
            }))
        );
    }

    #[test]
    fn a_non_text_or_multi_block_result_is_left_untouched() {
        let args = json!({"command": "x", "description": "x"});
        let reasoning = ToolResult {
            content: vec![ContentBlock::reasoning("unexpected")],
            is_error: false,
            meta: None,
        };
        assert_eq!(present(args.clone(), &reasoning), None);
        let two = ToolResult {
            content: vec![ContentBlock::text("a"), ContentBlock::text("b")],
            is_error: false,
            meta: None,
        };
        assert_eq!(present(args.clone(), &two), None);
        let none = ToolResult {
            content: vec![],
            is_error: false,
            meta: None,
        };
        assert_eq!(present(args, &none), None);
    }

    #[test]
    fn the_exit_parse_is_the_inverse_of_the_render_markers() {
        let base = |exit_code: Option<i64>, signal: Option<&str>, timed_out: bool| ShellRunResult {
            exit_code,
            signal: signal.map(str::to_owned),
            timed_out,
            aborted: false,
            timeout_ms: 1000.0,
            stdout: CollectedOutput {
                text: "out".into(),
                truncated: false,
                spill_path: None,
            },
            stderr: CollectedOutput {
                text: String::new(),
                truncated: false,
                spill_path: None,
            },
            sandbox: None,
        };
        let cases = [
            (base(Some(0), None, false), json!({"exitCode": 0})),
            (base(Some(7), None, false), json!({"exitCode": 7})),
            (
                base(None, Some("SIGTERM"), false),
                json!({"signal": "SIGTERM"}),
            ),
            // A trapped-timeout run that exits 0 has no signal/exit marker: it reads as exit 0.
            (base(Some(0), None, true), json!({"exitCode": 0})),
        ];
        for (result, expected) in cases {
            let rendered = render_result(
                &RenderedRun {
                    result: &result,
                    stopped: None,
                },
                &[],
            );
            let mut view = present(
                json!({"command": "x", "description": "x"}),
                &text_result(&rendered, false),
            )
            .unwrap();
            let output = view["output"].as_str().unwrap().to_owned();
            let fields = view.as_object_mut().unwrap();
            fields.remove("card");
            fields.remove("output");
            assert_eq!(Value::Object(fields.clone()), expected);
            assert!(!output.contains("[exit code: "));
            assert!(!output.contains("[killed by signal: "));
        }
    }

    #[test]
    fn the_description_names_the_managed_prefix_and_keeps_the_unset_variable_guard() {
        let text = bash_description();
        assert!(text.starts_with(
            "Execute a bash command (`bash -c`) and return its stdout/stderr. Each call runs in a fresh shell; pass `workdir` instead of using `cd`. Managed `$DSH_*` variables expose current harness environment facts. "
        ));
        assert!(text.contains("guard variables in such paths with `${VAR:?}`. "));
        assert!(text.ends_with("do not retry another way."));
        assert_eq!(DSH_ENV_PREFIX, "DSH_");
    }

    #[test]
    fn the_parameter_schema_follows_the_composition() {
        let keys = |value: &Value| {
            value
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            keys(&parameters(false, false, &[])),
            ["description", "command", "timeoutMs", "workdir"]
        );
        assert_eq!(
            keys(&parameters(true, true, &[])),
            [
                "description",
                "command",
                "timeoutMs",
                "workdir",
                "run_in_background"
            ]
        );
        let escalating = parameters(false, false, &[SandboxMode::WorkspaceWrite]);
        assert_eq!(
            keys(&escalating),
            [
                "description",
                "command",
                "timeoutMs",
                "workdir",
                "sandbox_permissions",
                "justification"
            ]
        );
        assert_eq!(
            escalating["justification"]["description"],
            "Required with sandbox_permissions: one sentence for the user explaining why this exact command needs the wider access. Use the language of the user’s current request."
        );
        assert_eq!(
            parameters(true, true, &[])["timeoutMs"]["description"],
            "Timeout in milliseconds. The executor applies its configured default and cap; on expiry the command moves to the background as a job instead of being killed."
        );
        assert_eq!(
            parameters(true, false, &[])["timeoutMs"]["description"],
            "Timeout in milliseconds. The executor applies its configured default and cap, and kills the command on expiry."
        );
    }
}
