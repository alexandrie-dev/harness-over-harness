use crate::RunnerFailureRule;
use hoh_util_values::js_trim;
use serde_json::Value;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerFailureMatch {
    pub detail: String,
}
pub async fn is_runner_spawn_failure(error: &Value, runner: Option<&str>, workdir: &str) -> bool {
    let Some(runner) = runner else {
        return false;
    };
    if !tokio::fs::metadata(workdir).await.is_ok_and(|m| m.is_dir()) {
        return false;
    }
    let path = workdir.to_owned();
    if !tokio::task::spawn_blocking(move || {
        rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok()
    })
    .await
    .unwrap_or(false)
    {
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
pub fn classify_runner_failure(
    exit_code: Option<i32>,
    stderr: &str,
    rules: &[RunnerFailureRule],
) -> Option<RunnerFailureMatch> {
    let code = exit_code.filter(|c| *c != 0)?;
    let lines: Vec<_> = stderr
        .split_inclusive('\n')
        .map(|l| {
            if let Some(line) = l.strip_suffix('\n') {
                line.strip_suffix('\r').unwrap_or(line)
            } else {
                l
            }
        })
        .collect();
    for rule in rules {
        if rule
            .allowed_exit_codes
            .as_ref()
            .is_some_and(|c| !c.contains(&code))
        {
            continue;
        }
        let infos: Vec<_> = rule
            .informational_lines
            .iter()
            .flatten()
            .map(|s| s.to_lowercase())
            .collect();
        let fatal: Vec<_> = rule
            .fatal_signatures
            .iter()
            .filter(|s| !js_trim(s).is_empty())
            .map(|s| s.to_lowercase())
            .collect();
        for line in &lines {
            let lower = line.to_lowercase();
            if infos.contains(&lower) {
                continue;
            }
            if fatal.iter().any(|s| lower.contains(s)) {
                return Some(RunnerFailureMatch {
                    detail: (*line).into(),
                });
            }
        }
    }
    None
}
pub fn matches_signature(exit_code: Option<i32>, stderr: &str, signatures: &[String]) -> bool {
    exit_code.is_some_and(|c| c != 0)
        && signatures
            .iter()
            .any(|s| stderr.to_lowercase().contains(&s.to_lowercase()))
}
