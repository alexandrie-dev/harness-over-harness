//! Model-facing result rendering for the bash tool.

use hoh_sandbox::{SandboxMode, escalation_hint_marker, sandbox_denial_marker};
use hoh_shell::{CollectedOutput, ShellRunResult, ShellSandboxInfo};
use hoh_util_values::js_number_string;

/// The exit-status parse is the shared marker-contract half of the shell-tool rendering story,
/// owned by `hoh-shell` so a PowerShell tool reuses it; re-exported to keep this module a single
/// import root for bash-tool consumers.
pub use hoh_shell::{ParsedExitStatus, parse_exit_status};

/// Append the truncation notice (with the full-output spill path) to a stream's text.
fn stream_text(output: &CollectedOutput) -> String {
    if !output.truncated {
        return output.text.clone();
    }
    format!(
        "{}\n[output truncated; full output: {}]",
        output.text,
        output.spill_path.as_deref().unwrap_or("(unavailable)")
    )
}

/// A completed foreground run plus the optional `stopped` reason of an outside kill.
pub struct RenderedRun<'a> {
    pub result: &'a ShellRunResult,
    pub stopped: Option<&'a str>,
}

/// Shape one finished run into the text the model sees: stdout, then a marked stderr section, then
/// exit-status markers. Non-zero exits are reported, not errored — the model decides how to react;
/// only infrastructure failures (spawn errors, aborts) surface as error results.
///
/// `escalation_modes` is the escalation targets this composition advertises; non-empty adds the
/// same-turn escalation hint after a denial marker.
pub fn render_result(run: &RenderedRun<'_>, escalation_modes: &[SandboxMode]) -> String {
    let result = run.result;
    let out = stream_text(&result.stdout);
    let err = stream_text(&result.stderr);

    let mut body = out;
    if !err.is_empty() {
        // Single newline between sections (stdout usually ends with one already).
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("[stderr]\n");
        body.push_str(&err);
    }
    if body.is_empty() {
        body = "(no output)".to_owned();
    }

    let mut markers: Vec<String> = Vec::new();
    // Keep the exit marker last because parse_exit_status anchors there.
    if let Some(sandbox) = result.sandbox.as_ref().filter(|sandbox| sandbox.denied) {
        markers.push(sandbox_denial_marker(sandbox.mode));
        // Hint only when the composition exposes escalation, before the final exit marker.
        if !escalation_modes.is_empty() {
            markers.push(escalation_hint_marker("command"));
        }
    }
    // A command may trap SIGTERM and exit 0 after timeout; still report interruption.
    if result.timed_out {
        markers.push(format!(
            "[timed out after {}ms]",
            js_number_string(result.timeout_ms)
        ));
    }
    // A kill from outside the call (the human stopping its job) is not a command failure: the
    // reason tells the model not to retry.
    if let Some(stopped) = run.stopped {
        markers.push(format!("[stopped: {stopped}]"));
    }
    if let Some(signal) = &result.signal {
        markers.push(format!("[killed by signal: {signal}]"));
    } else if result.exit_code != Some(0) {
        markers.push(format!("[exit code: {}]", exit_code_text(result.exit_code)));
    }
    if markers.is_empty() {
        return body;
    }

    if !body.ends_with('\n') {
        body.push('\n');
    }
    body + &markers.join("\n")
}

/// `${result.exitCode}` of a number or `null`.
fn exit_code_text(exit_code: Option<i64>) -> String {
    exit_code.map_or_else(|| "null".to_owned(), |code| code.to_string())
}

/// Shape a foreground call that stopped waiting into the text the model sees: the output captured so
/// far (one consuming registry read taken at that point, so `job_output` continues exactly after
/// it), then the still-running marker.
pub fn render_promoted(job_id: &str, timeout_ms: f64, output: &str) -> String {
    let body = if output.is_empty() {
        String::new()
    } else if output.ends_with('\n') {
        output.to_owned()
    } else {
        format!("{output}\n")
    };
    format!(
        "{body}[still running after {}ms; moved to background job {job_id}]",
        js_number_string(timeout_ms)
    )
}

/// Shape the one consuming registry read a foreground call embeds in its result when it stops
/// waiting: the output produced so far, plus the dropped-output notice (naming the job's spill files)
/// when the model cursor fell behind the ring, and the sandbox notices.
pub fn render_job_read(
    delta: &str,
    lossy: bool,
    spill_paths: &[String],
    sandbox: Option<&ShellSandboxInfo>,
    escalation_modes: &[SandboxMode],
) -> String {
    let mut notices: Vec<String> = Vec::new();
    if lossy {
        let paths = if spill_paths.is_empty() {
            "(unavailable)".to_owned()
        } else {
            spill_paths.join(", ")
        };
        notices.push(format!(
            "[some output was dropped from memory; full output: {paths}]"
        ));
    }
    if let Some(sandbox) = sandbox {
        if sandbox.runner_failed == Some(true) {
            notices.push(runner_failed_notice(sandbox.mode));
        } else if sandbox.denied {
            notices.push(sandbox_denial_marker(sandbox.mode));
            if !escalation_modes.is_empty() {
                notices.push(escalation_hint_marker("command"));
            }
        }
    }
    if notices.is_empty() {
        return delta.to_owned();
    }
    let separator = if !delta.is_empty() && !delta.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    format!("{delta}{separator}{}", notices.join("\n"))
}

/// The notice of a sandbox runner that failed before running the command.
pub(crate) fn runner_failed_notice(mode: SandboxMode) -> String {
    format!(
        "[sandbox: the sandbox runner itself failed under {mode} mode — the command did not run; this is a sandbox problem, not a command failure]"
    )
}
