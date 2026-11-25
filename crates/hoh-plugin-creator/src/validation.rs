//! Run the same metadata checks as the checked-in index generator.

use serde::{Deserialize, Serialize};
use std::{
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

const VALIDATOR: &str = include_str!("../../../scripts/generate-plugin-index.py");
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Issue {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Issue {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            file: None,
            line: None,
            hint: None,
        }
    }

    pub(crate) fn at(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(out, "{}", file.display())?;
            if let Some(line) = self.line {
                write!(out, ":{line}")?;
            }
            write!(out, ": ")?;
        }
        write!(out, "{}", self.message)?;
        if let Some(hint) = &self.hint {
            write!(out, " {hint}")?;
        }
        Ok(())
    }
}
impl std::error::Error for Issue {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    format_version: u32,
    pub issues: Vec<Issue>,
    pub manifest: Option<String>,
}

pub(crate) fn preflight(root: &Path, operation: &str, id: &str) -> Result<Report, Issue> {
    let output = Command::new("python3")
        .args(["-I", "-c", VALIDATOR, "--json", "--root"])
        .arg(root)
        .arg(format!("{operation}={id}"))
        .output()
        .map_err(|error| {
            Issue::new("validator_unavailable", error.to_string())
                .with_hint("Use Python 3.11 or later on PATH.")
        })?;
    let report = serde_json::from_slice::<Report>(&output.stdout).ok();
    match report {
        Some(report)
            if report.format_version == FORMAT_VERSION
                && (output.status.success() && report.issues.is_empty()
                    || output.status.code() == Some(1) && !report.issues.is_empty()) =>
        {
            Ok(report)
        }
        _ => Err(Issue::new(
            "validator_failed",
            "Plugin metadata validator returned an invalid response.",
        )
        .with_hint("Check that Python 3.11 or later is on PATH.")),
    }
}
