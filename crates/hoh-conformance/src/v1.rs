//! Run of the V1-SCENARIOS.tsv selection: one status per scenario, written outside the repository.
use crate::{Result, read, runner::Outcome};
use std::{fs, path::Path};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub suite: String,
    pub scenario: String,
    pub group: String,
}

/// Rows of the list whose `v1` column is `group` (`in`, `extra`; `all` takes both).
/// The header and `#` comment lines are skipped; ACP rows are always `out`, so they never appear.
pub fn rows(list: &str, group: &str) -> Result<Vec<Row>> {
    if !["in", "extra", "all"].contains(&group) {
        return Err(format!("invalid group {group}: use in, extra or all"));
    }
    let mut result = Vec::new();
    for line in list.lines() {
        if line.starts_with('#') || line.starts_with("suite\t") || line.trim().is_empty() {
            continue;
        }
        let cells: Vec<_> = line.split('\t').collect();
        if cells.len() < 3 {
            return Err(format!("malformed list row: {line}"));
        }
        let v1 = cells[2];
        if (group == "all" && ["in", "extra"].contains(&v1)) || v1 == group {
            result.push(Row {
                suite: cells[0].into(),
                scenario: cells[1].into(),
                group: v1.into(),
            });
        }
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Passed,
    Diverged,
    NotStarted,
}
impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Passed => "прошло",
            Self::Diverged => "расхождение",
            Self::NotStarted => "не запустилось",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Verdict {
    pub status: Status,
    pub phase: String,
    /// Controller checks of the TS scenario body that the runner has not ported (`-` when none).
    pub body_check: String,
    pub detail: String,
}

/// `stderr` is the process's stderr text. A process that never booted its profile did not start;
/// a started process whose recording differs from the replay diverged.
///
/// `scenario-check` is the runner's last gate: it is reached only after every comparison
/// (exit, stdout, workspace, log, headers, prompt, tool schemas) matched, and names the TS body
/// checks that are not ported. Those scenarios are `Passed` with the check listed in `body_check`.
pub fn classify(outcome: &Outcome, stderr: &str) -> Verdict {
    let line = |s: &str| s.lines().next().unwrap_or("").replace(['\t', '\r'], " ");
    if let Some(error) = &outcome.runner_error {
        return Verdict {
            status: Status::NotStarted,
            phase: "runner".into(),
            body_check: "-".into(),
            detail: line(error),
        };
    }
    let Some(failure) = &outcome.failure else {
        return Verdict {
            status: Status::Passed,
            phase: "-".into(),
            body_check: "-".into(),
            detail: String::new(),
        };
    };
    if failure.phase == "scenario-check" {
        return Verdict {
            status: Status::Passed,
            phase: "-".into(),
            body_check: line(&failure.reason),
            detail: String::new(),
        };
    }
    let booted_nothing = stderr.contains("startup failed");
    let status = if ["input", "setup", "launch"].contains(&failure.phase) || booted_nothing {
        Status::NotStarted
    } else {
        Status::Diverged
    };
    let detail = if booted_nothing && !stderr.is_empty() {
        let plugins = stderr
            .lines()
            .filter(|l| l.trim_start().starts_with("Package:"))
            .count();
        format!(
            "{}; startup failed ({plugins} plugin rows)",
            line(&failure.reason)
        )
    } else {
        line(&failure.reason)
    };
    Verdict {
        status,
        phase: failure.phase.into(),
        body_check: "-".into(),
        detail,
    }
}

pub struct Entry {
    pub row: Row,
    pub verdict: Verdict,
}

pub fn summary_tsv(entries: &[Entry]) -> String {
    let mut text = String::from("suite\tscenario\tgroup\tstatus\tphase\tbody_check\tdetail\n");
    for e in entries {
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            e.row.suite,
            e.row.scenario,
            e.row.group,
            e.verdict.status.label(),
            e.verdict.phase,
            e.verdict.body_check,
            e.verdict.detail
        ));
    }
    text
}

/// «прошло A, расхождение B, не запустилось C» per group.
pub fn totals(entries: &[Entry]) -> String {
    let mut lines = Vec::new();
    for group in ["in", "extra"] {
        let of = |s: Status| {
            entries
                .iter()
                .filter(|e| e.row.group == group && e.verdict.status == s)
                .count()
        };
        let total = entries.iter().filter(|e| e.row.group == group).count();
        if total > 0 {
            let unchecked = entries
                .iter()
                .filter(|e| {
                    e.row.group == group
                        && e.verdict.status == Status::Passed
                        && e.verdict.body_check != "-"
                })
                .count();
            lines.push(format!(
                "{group} {total}: прошло {} (тело не проверено {unchecked}), расхождение {}, не запустилось {}",
                of(Status::Passed),
                of(Status::Diverged),
                of(Status::NotStarted)
            ));
        }
    }
    lines.join("\n")
}

/// Copy the per-scenario evidence next to the summary: `diffs/<suite>/<scenario>.diff` and, for a
/// process that did not start, `diffs/<suite>/<scenario>.stderr`.
pub fn write_evidence(out: &Path, runs: &Path, entry: &Entry) -> Result<()> {
    if entry.verdict.status == Status::Passed {
        return Ok(());
    }
    let dir = out.join("diffs").join(&entry.row.suite);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let scenario = runs.join(&entry.row.suite).join(&entry.row.scenario);
    let latest = read(&scenario.join("latest-run.txt")).unwrap_or_default();
    let attempt = Path::new(latest.trim());
    for (source, extension) in [
        (attempt.join("diff.txt"), "diff"),
        (attempt.join("stderr.txt"), "stderr"),
    ] {
        if let Ok(text) = fs::read_to_string(&source)
            && !text.is_empty()
        {
            fs::write(
                dir.join(format!("{}.{extension}", entry.row.scenario)),
                text,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn stderr_of(runs: &Path, row: &Row) -> String {
    let scenario = runs.join(&row.suite).join(&row.scenario);
    read(&scenario.join("latest-run.txt"))
        .ok()
        .and_then(|latest| fs::read_to_string(Path::new(latest.trim()).join("stderr.txt")).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::Failure;
    use serde_json::json;

    const LIST: &str = "# comment\nsuite\tscenario\tv1\treason\tnote\nsession\ta\tin\t-\t-\nsession\tb\tout\tx\t-\nsdk\tc\textra\t-\t-\nacp\td\tout\tACP\t-\n";

    fn outcome(failure: Option<Failure>, error: Option<&str>) -> Outcome {
        Outcome {
            suite: "session".into(),
            scenario: "a".into(),
            failure,
            runner_error: error.map(str::to_owned),
        }
    }

    #[test]
    fn rows_select_the_requested_group_and_skip_comments() {
        let names = |g| {
            rows(LIST, g)
                .unwrap()
                .iter()
                .map(|r| format!("{}/{}", r.suite, r.scenario))
                .collect::<Vec<_>>()
        };
        assert_eq!(names("in"), ["session/a"]);
        assert_eq!(names("extra"), ["sdk/c"]);
        assert_eq!(names("all"), ["session/a", "sdk/c"]);
        assert!(rows(LIST, "out").is_err());
    }

    #[test]
    fn a_profile_that_did_not_boot_is_not_started_even_after_the_exit_phase() {
        let stderr = "hoh: startup failed: 1 required plugin did not activate\n  Package: x\n";
        let verdict = classify(
            &outcome(Some(Failure::new("exit", "exit Some(1); expected 0")), None),
            stderr,
        );
        assert_eq!(verdict.status, Status::NotStarted);
        assert!(verdict.detail.contains("1 plugin rows"));
    }

    #[test]
    fn a_comparison_failure_after_a_clean_boot_is_a_divergence() {
        let verdict = classify(
            &outcome(Some(Failure::new("log", "event 3 differs")), None),
            "",
        );
        assert_eq!(verdict.status, Status::Diverged);
        assert_eq!(verdict.phase, "log");
    }

    #[test]
    fn the_last_gate_means_every_comparison_matched_and_names_the_unported_check() {
        let verdict = classify(
            &outcome(
                Some(Failure::new(
                    "scenario-check",
                    "check verifyAdvancedToolchain not implemented",
                )),
                None,
            ),
            "",
        );
        assert_eq!(verdict.status, Status::Passed);
        assert_eq!(
            verdict.body_check,
            "check verifyAdvancedToolchain not implemented"
        );
    }

    #[test]
    fn a_failed_body_check_is_a_divergence_and_keeps_its_diff() {
        let failed = outcome(
            Some(Failure::new("body", "working-directory/change mismatch")),
            None,
        );
        let verdict = classify(&failed, "");
        assert_eq!(verdict.status, Status::Diverged);
        assert_eq!(verdict.phase, "body");
        assert_eq!(verdict.body_check, "-");
        // The unported-check stub is still the only thing that leaves a scenario `Passed`.
        let stub = outcome(
            Some(Failure::new(
                "scenario-check",
                "check verifyWorktree not implemented",
            )),
            None,
        );
        assert_eq!(classify(&stub, "").status, Status::Passed);
        // And a body failure raised by the real body helpers carries the `body` phase.
        let helper = crate::body::expect_eq("label", &json!(1), &json!(2)).unwrap_err();
        assert_eq!(helper.phase, "body");
        assert_eq!(crate::body::fail("x").phase, "body");
    }

    #[test]
    fn a_runner_error_and_a_setup_failure_are_not_started() {
        assert_eq!(
            classify(&outcome(None, Some("boom")), "").status,
            Status::NotStarted
        );
        assert_eq!(
            classify(&outcome(Some(Failure::new("setup", "x")), None), "").status,
            Status::NotStarted
        );
    }

    #[test]
    fn totals_count_each_group_separately() {
        let entry = |group: &str, status| Entry {
            row: Row {
                suite: "session".into(),
                scenario: "s".into(),
                group: group.into(),
            },
            verdict: Verdict {
                status,
                phase: "-".into(),
                body_check: "-".into(),
                detail: String::new(),
            },
        };
        let entries = [
            entry("in", Status::Passed),
            entry("in", Status::Diverged),
            entry("extra", Status::NotStarted),
        ];
        assert_eq!(
            totals(&entries),
            "in 2: прошло 1 (тело не проверено 0), расхождение 1, не запустилось 0\nextra 1: прошло 0 (тело не проверено 0), расхождение 0, не запустилось 1"
        );
    }
}
