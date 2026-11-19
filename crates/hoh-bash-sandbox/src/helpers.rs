//! Shell-result projection over shared sandbox diagnostics.
use hoh_shell::ShellRunResult;
use serde_json::Value;

pub use hoh_sandbox::{RunnerFailureMatch, classify_runner_failure, matches_signature};

/// Classify a failed run against the selected backend's denial dialect.
///
/// `result` is the settled foreground run and `signatures` the case-insensitive denial substrings
/// from the active wrap. Returns whether the failed run matches that denial dialect.
pub fn classify_denial(result: &ShellRunResult, signatures: &[String]) -> bool {
    matches_signature(
        result.exit_code.and_then(|code| i32::try_from(code).ok()),
        &result.stderr.text,
        signatures,
    )
}

/// Whether the caller-owned spawn cwd can be entered (`statSync` + `accessSync(X_OK)`).
fn is_usable_workdir(path: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_dir())
        && rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok()
}

/// Attribute only ENOENT/EACCES failures whose error path equals argv[0] after independently
/// ruling out the caller-owned cwd. A supplied error path must exactly identify the runner;
/// without one, the syscall must. With a usable cwd, these codes describe resolution or execute
/// permission for that argv[0] or its shebang interpreter.
///
/// Synchronous like the source (`statSync`/`accessSync`): the settlement hook that needs it is
/// synchronous, so it does not go through the async `hoh_sandbox::is_runner_spawn_failure`. The
/// workdir is checked at classification time, not atomically with spawn; concurrent path
/// replacement may change attribution but cannot permit an unconfined execution.
///
/// `error` carries the Node-shaped `code`/`syscall`/`path` of the original spawn rejection.
pub fn is_runner_spawn_failure(error: &Value, runner_program: Option<&str>, workdir: &str) -> bool {
    let Some(runner) = runner_program else {
        return false;
    };
    if !is_usable_workdir(workdir) {
        return false;
    }
    if !matches!(
        error.get("code").and_then(Value::as_str),
        Some("EACCES" | "ENOENT")
    ) {
        return false;
    }
    let Some(syscall) = error.get("syscall").and_then(Value::as_str) else {
        return false;
    };
    let exact = format!("spawn {runner}");
    match error.get("path") {
        None => syscall == exact,
        Some(Value::String(path)) => {
            !path.is_empty() && path == runner && (syscall == "spawn" || syscall == exact)
        }
        _ => false,
    }
}
