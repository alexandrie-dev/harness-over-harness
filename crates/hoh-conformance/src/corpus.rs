use crate::{Result, read, records};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Scenario {
    pub suite: String,
    pub name: String,
    pub dir: PathBuf,
    pub manifest: Value,
}

impl Scenario {
    pub fn profile(&self) -> &str {
        self.manifest["profile"].as_str().unwrap_or("")
    }
    pub fn composition(&self) -> &str {
        self.manifest["composition"].as_str().unwrap_or("")
    }
    pub fn fixtures(&self) -> Result<Vec<PathBuf>> {
        fixture_files(&self.dir)
    }
    pub fn expected_logs(&self) -> Result<Vec<String>> {
        let files = self.fixtures()?;
        files
            .iter()
            .enumerate()
            .map(|(i, file)| {
                if self.manifest.get("sessionFormat").is_some() {
                    read(&self.dir.join(if i == 0 {
                        "writer.expected.jsonl".into()
                    } else {
                        format!("writer.{i}.expected.jsonl")
                    }))
                } else {
                    read(file)
                }
            })
            .collect()
    }
}

pub fn discover(refs: &Path) -> Result<Vec<Scenario>> {
    let mut all = Vec::new();
    for suite in ["session", "sdk", "acp"] {
        for entry in sorted_entries(&refs.join("snapshots").join(suite))? {
            let dir = entry.path();
            let file = dir.join("snapshot.yml");
            if !file.is_file() {
                continue;
            }
            let manifest = crate::manifest::parse(&read(&file)?, &file.to_string_lossy())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // `scenario` is optional in the source; when present it names the directory.
            if manifest
                .get("scenario")
                .is_some_and(|scenario| *scenario != name)
            {
                return Err(format!("{}: scenario name mismatch", file.display()));
            }
            all.push(Scenario {
                suite: suite.into(),
                name,
                dir,
                manifest,
            });
        }
    }
    Ok(all)
}

pub fn sorted_entries(dir: &Path) -> Result<Vec<std::fs::DirEntry>> {
    let mut entries = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(|e| e.file_name());
    Ok(entries)
}

pub fn fixture_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let re = regex::Regex::new(r"^session(?:\.([1-9][0-9]*))?(?:\.v([1-9][0-9]*))?\.jsonl$")
        .map_err(|e| e.to_string())?;
    let mut selected: BTreeMap<usize, (u64, PathBuf)> = BTreeMap::new();
    for entry in sorted_entries(dir)? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(c) = re.captures(&name) else {
            if name.starts_with("session") && name.ends_with(".jsonl") {
                return Err(format!("invalid session fixture name: {name}"));
            }
            continue;
        };
        let index = c
            .get(1)
            .map_or(Ok(0), |m| m.as_str().parse::<usize>())
            .map_err(|e| e.to_string())?;
        let version = c
            .get(2)
            .map_or(Ok(0), |m| m.as_str().parse::<u64>())
            .map_err(|e| e.to_string())?;
        if version > 9_007_199_254_740_991 {
            return Err("invalid fixture generation".into());
        }
        if selected.get(&index).is_none_or(|(v, _)| version > *v) {
            selected.insert(index, (version, entry.path()));
        }
    }
    if !selected.contains_key(&0) {
        return Err("missing parent session fixture".into());
    }
    let mut result = Vec::new();
    for (offset, (index, (version, path))) in selected.into_iter().enumerate() {
        if offset != index {
            return Err(format!(
                "session fixture roles must be contiguous: expected {offset}, found {index}"
            ));
        }
        let rows = records(&read(&path)?)?;
        let header = rows.first().ok_or("empty fixture")?;
        if header["type"] != "session"
            || header.get("version").map_or(Some(0), Value::as_u64) != Some(version)
        {
            return Err(format!(
                "{}: filename/header version mismatch",
                path.display()
            ));
        }
        result.push(path);
    }
    Ok(result)
}

pub fn source<'a>(
    all: &'a [Scenario],
    scenario: &'a Scenario,
    field: &str,
) -> Result<&'a Scenario> {
    let Some(name) = scenario.manifest["header"][field].as_str() else {
        return Ok(scenario);
    };
    let (suite, name) = name.split_once('/').unwrap_or((&scenario.suite, name));
    all.iter()
        .find(|s| s.suite == suite && s.name == name)
        .ok_or_else(|| format!("missing satellite source {suite}/{name}"))
}

pub fn pin<'a>(all: &'a [Scenario], scenario: &Scenario) -> Result<&'a Scenario> {
    all.iter()
        .find(|s| {
            (s.suite == scenario.suite || scenario.suite == "sdk" && s.suite == "session")
                && s.composition() == scenario.composition()
                && s.manifest["header"]["class"] == scenario.manifest["header"]["class"]
                && s.manifest["header"]["pin"] == true
        })
        .ok_or_else(|| {
            format!(
                "missing header pin for {}/{}",
                scenario.suite, scenario.name
            )
        })
}

pub fn patch_sources(all: &[Scenario], scenario: &Scenario, refs: &Path) -> Result<Vec<PathBuf>> {
    if scenario.suite == "acp" {
        return Ok(vec![
            refs.join("snapshots/acp/escalation-approved/cordis.yml"),
            refs.join(if scenario.composition() == "image-compaction" {
                "snapshots/acp/image-compaction/cordis.snapshot.yml"
            } else {
                "snapshots/acp/escalation-approved/cordis.snapshot.yml"
            }),
        ]);
    }
    let owner = all
        .iter()
        .find(|s| {
            s.suite != "acp"
                && s.composition() == scenario.composition()
                && s.dir.join("cordis.yml").is_file()
        })
        .ok_or("missing composition owner")?;
    let mut paths = Vec::new();
    if scenario.suite == "sdk"
        && [
            "default",
            "dynamic-tool-updates",
            "image",
            "ptc",
            "sdk-account-provider-signout",
            "sdk-default",
            "sdk-dsh-sdk-diagnostic",
            "sdk-dsh-sdk-dynamic-route",
            "sdk-minimal",
            "sdk-serial-created",
            "sdk-team-session-mode",
            "sdk-upload",
            "session-title",
            "session-title-stability",
            "subagent-activation-limit",
            "subagent-continuable-inheritance",
            "subagent-durability-failure",
            "subagent-send-message",
            "system-prompt-in-history",
        ]
        .contains(&scenario.composition())
    {
        paths.push(refs.join("snapshots/sdk/legacy-context.patch.yml"));
    }
    if scenario.suite == "sdk" && scenario.composition().starts_with("sdk-") {
        paths.extend([
            owner.dir.join("cordis.yml"),
            owner.dir.join("cordis.snapshot.yml"),
        ]);
    } else {
        paths.extend([
            refs.join("snapshots/session/text-turn/cordis.yml"),
            owner.dir.join("cordis.snapshot.yml"),
            refs.join("snapshots/session/text-turn/model.cordis.yml"),
        ]);
    }
    if scenario.suite == "session" {
        if scenario.dir.join("runtime.cordis.yml").is_file() {
            paths.push(scenario.dir.join("runtime.cordis.yml"));
        }
    } else {
        match scenario.name.as_str() {
            "team-default-enabled" => paths.extend([
                refs.join("apps/cli/config/examples/team-mode.yml"),
                scenario.dir.join("capture.cordis.yml"),
            ]),
            "manual-compact"
            | "permission-switch"
            | "plan-review"
            | "goal-rounds"
            | "command-attachments" => paths.push(scenario.dir.join("capture.cordis.yml")),
            "subagent-fork-in-process" | "subagent-mixed" | "subagent-continuable-inheritance" => {
                paths.push(refs.join("snapshots/sdk/subagent-spawn-in-process/runtime.cordis.yml"))
            }
            "approval"
            | "cua-run-code"
            | "command-identity"
            | "tool-scheduler-recovery"
            | "tool-error-details"
            | "ptc-cancel"
            | "ptc-turn"
            | "subagent-spawn-in-process" => paths.push(scenario.dir.join("runtime.cordis.yml")),
            "session-resume" => paths.push(scenario.dir.join("replay.cordis.yml")),
            _ => {}
        }
    }
    Ok(paths)
}
