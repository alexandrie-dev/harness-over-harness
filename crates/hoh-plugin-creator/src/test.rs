//! Offline package tests with compact compiler diagnostics and a full failure log.

use crate::{FORMAT_VERSION, Issue, PluginCreator, repository};
use serde::Serialize;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestReport {
    pub format_version: u32,
    pub id: String,
    pub ok: bool,
    pub issues: Vec<Issue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<PathBuf>,
}

impl PluginCreator {
    pub fn test(&self, cwd: &Path, id: &str) -> Result<TestReport, Issue> {
        let inspection = self.inspect(cwd, id, &[])?;
        let mut report = TestReport {
            format_version: FORMAT_VERSION,
            id: id.into(),
            ok: false,
            issues: inspection.issues,
            log: None,
        };
        if !report.issues.is_empty() {
            return Ok(report);
        }
        let root = repository(cwd)?;
        let log = tempfile::Builder::new()
            .prefix("hoh-plugin-test-")
            .suffix(".log")
            .tempfile()
            .map_err(log_error)?;
        let stderr = std::fs::OpenOptions::new()
            .append(true)
            .open(log.path())
            .map_err(log_error)?;
        // Use append mode for both writers so bytes already logged on stderr are retained.
        let mut stdout_log = stderr.try_clone().map_err(log_error)?;
        let mut child = Command::new("cargo")
            .current_dir(root)
            .args([
                "test",
                "--offline",
                "--package",
                &format!("hoh-plugin-{id}"),
                "--message-format=json",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .map_err(|error| {
                Issue::new("cargo_unavailable", error.to_string())
                    .with_hint("Put the HoH Rust toolchain and Cargo on PATH.")
            })?;
        let mut stdout = BufReader::new(child.stdout.take().expect("Cargo stdout is piped"));
        let collect = (|| {
            let mut line = Vec::new();
            while stdout.read_until(b'\n', &mut line)? != 0 {
                stdout_log.write_all(&line)?;
                if report.issues.len() < 3
                    && let Some(issue) = cargo_issue(&line).or_else(|| failed_test(&line))
                    && !report.issues.contains(&issue)
                {
                    report.issues.push(issue);
                }
                line.clear();
            }
            Ok::<_, std::io::Error>(())
        })();
        if let Err(error) = collect {
            let _ = child.kill();
            let _ = child.wait();
            return Err(log_error(error));
        }
        let status = child
            .wait()
            .map_err(|error| Issue::new("cargo_wait_failed", error.to_string()))?;
        report.ok = status.success();
        if report.ok {
            report.issues.clear();
        } else {
            if report.issues.is_empty() {
                report.issues.push(
                    Issue::new("test_failed", format!("Cargo test exited with {status}."))
                        .with_hint("See the log for the first failing test or Cargo error."),
                );
            }
            let (_, path) = log.keep().map_err(|error| log_error(error.error))?;
            report.log = Some(path);
        }
        Ok(report)
    }
}

fn cargo_issue(line: &[u8]) -> Option<Issue> {
    let message: Value = serde_json::from_slice(line).ok()?;
    let diagnostic = &message["message"];
    if message["reason"] != "compiler-message" || diagnostic["level"] != "error" {
        return None;
    }
    let text = diagnostic["message"].as_str()?;
    let mut issue = Issue::new(
        "compile_error",
        if text.chars().count() > 320 {
            format!("{}…", text.chars().take(320).collect::<String>())
        } else {
            text.into()
        },
    );
    if let Some(span) = diagnostic["spans"]
        .as_array()?
        .iter()
        .find(|span| span["is_primary"] == true)
    {
        issue.file = span["file_name"].as_str().map(PathBuf::from);
        issue.line = span["line_start"].as_u64();
    }
    Some(issue)
}

fn failed_test(line: &[u8]) -> Option<Issue> {
    let name = std::str::from_utf8(line)
        .ok()?
        .trim()
        .strip_prefix("test ")?
        .strip_suffix(" ... FAILED")?;
    let name: String = name.chars().take(200).collect();
    Some(
        Issue::new("smoke_test_failed", format!("Test {name} failed."))
            .with_hint("Read the assertion details in the log."),
    )
}

fn log_error(error: std::io::Error) -> Issue {
    Issue::new("test_log_failed", error.to_string())
}
