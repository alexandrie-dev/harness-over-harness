use crate::{
    Result,
    corpus::{Scenario, sorted_entries},
};
use base64::Engine;
use serde_json::{Value, json};
use std::{fs, path::Path};

pub fn ignored(s: &Scenario) -> Vec<&'static str> {
    match s.suite.as_str() {
        "sdk" => vec![
            ".agents",
            ".child-dsh",
            ".dsh",
            ".replay-fixtures",
            ".snapshot-patches",
        ],
        "acp" => vec![
            ".agents",
            ".dsh",
            ".dsh-profile-patches",
            ".dsh-snapshot-stream-ready",
        ],
        _ if s.manifest["workspace"]["setup"] == "git-worktree" => {
            vec![".dsh", ".snapshot-patches", ".git"]
        }
        _ => vec![".agents", ".dsh", ".snapshot-patches"],
    }
}

pub fn capture(root: &Path, ignored: &[&str], expected: bool) -> Result<Vec<Value>> {
    fn visit(root: &Path, relative: &Path, ignored: &[&str], expected: bool) -> Result<Vec<Value>> {
        let mut result = Vec::new();
        for entry in sorted_entries(&root.join(relative))? {
            let name = entry.file_name();
            if relative.as_os_str().is_empty()
                && (ignored.iter().any(|s| name == *s) || expected && name == ".empty")
            {
                continue;
            }
            let path = relative.join(name);
            let label = path.to_string_lossy().replace('\\', "/");
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                result.push(json!({"path":label,"kind":"symlink","target":fs::read_link(entry.path()).map_err(|e|e.to_string())?.to_string_lossy()}));
            } else if kind.is_dir() {
                let children = visit(root, &path, ignored, expected)?;
                if children.is_empty() {
                    result.push(json!({"path":label,"kind":"empty-directory"}));
                } else {
                    result.extend(children);
                }
            } else if kind.is_file() {
                if expected && path.file_name().is_some_and(|s| s == ".empty") {
                    result.push(json!({"path":relative.to_string_lossy().replace('\\',"/"),"kind":"empty-directory"}));
                    continue;
                }
                let bytes = fs::read(entry.path()).map_err(|e| e.to_string())?;
                if !bytes.contains(&0)
                    && let Ok(text) = std::str::from_utf8(&bytes)
                {
                    result.push(json!({"path":label,"kind":"text","content":text}));
                    continue;
                }
                result.push(json!({"path":label,"kind":"binary","base64":base64::engine::general_purpose::STANDARD.encode(bytes)}));
            } else {
                return Err(format!(
                    "unsupported filesystem entry {}",
                    entry.path().display()
                ));
            }
        }
        Ok(result)
    }
    visit(root, Path::new(""), ignored, expected)
}

pub fn copy(source: &Path, target: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(source).map_err(|e| format!("{}: {e}", source.display()))?;
    if meta.file_type().is_symlink() {
        let link = fs::read_link(source).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(link, target).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        {
            if source.is_dir() {
                std::os::windows::fs::symlink_dir(link, target)
            } else {
                std::os::windows::fs::symlink_file(link, target)
            }
            .map_err(|e| e.to_string())?;
        }
    } else if meta.is_dir() {
        fs::create_dir_all(target).map_err(|e| e.to_string())?;
        for e in sorted_entries(source)? {
            copy(&e.path(), &target.join(e.file_name()))?;
        }
        fs::set_permissions(target, meta.permissions()).map_err(|e| e.to_string())?;
    } else {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::copy(source, target).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, text).map_err(|e| e.to_string())
}

/// Returns an explicit unsatisfied setup check, never silently treating it as prepared.
pub fn prepare(s: &Scenario, cwd: &Path, refs: &Path) -> Result<Option<String>> {
    let source = s.dir.join("workspace");
    if source.is_dir() {
        for e in sorted_entries(&source)? {
            copy(&e.path(), &cwd.join(e.file_name()))?;
        }
    }
    let Some(setup) = s.manifest["workspace"]["setup"].as_str() else {
        return Ok(None);
    };
    match setup {
        "office-skills" | "office-retired-ptc" => {
            copy(
                &refs.join("packages/skill/skill-office/assets"),
                &cwd.join("office-skills"),
            )?;
            if setup == "office-retired-ptc" {
                // Resolve the required host tool without executing Node in this writer lane.
                let node = std::env::var_os("PATH")
                    .into_iter()
                    .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
                    .map(|p| p.join(if cfg!(windows) { "node.exe" } else { "node" }))
                    .find(|p| p.is_file())
                    .ok_or("office-retired-ptc requires node on PATH")?;
                let node = fs::canonicalize(node)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\'', "'\\''");
                write(
                    &cwd.join("office-node"),
                    &format!("#!/bin/sh\nexec '{node}' \"$@\"\n"),
                )?;
                write(
                    &cwd.join("office-ptc-node"),
                    "#!/bin/sh\nexec \"$(dirname \"$0\")/office-node\" --stack-trace-limit=0 \"$@\"\n",
                )?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    for name in ["office-node", "office-ptc-node"] {
                        fs::set_permissions(cwd.join(name), fs::Permissions::from_mode(0o755))
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
        }
        "dsh-doc-resources" => copy(
            &refs.join("packages/preset/agent-preset/skills/dsh-doc"),
            &cwd.join(".dsh/skills/dsh-doc"),
        )?,
        "windows-acl-skill" => copy(
            &refs.join(
                "packages/sandbox/sandbox-windows-acl/assets/diagnose-windows-sandbox-acl/SKILL.md",
            ),
            &cwd.join(".dsh/skills/diagnose-windows-sandbox-acl/SKILL.md"),
        )?,
        "skill-root-isolation" => {
            copy(&refs.join("snapshots/session/skill-load/workspace/.dsh/skills/editing-cordis-compositions/SKILL.md"),&cwd.join(".dsh/skills/editing-cordis-compositions/SKILL.md"))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink("broken-skills", cwd.join("broken-skills"))
                .map_err(|e| e.to_string())?;
            #[cfg(not(unix))]
            return Ok(Some(
                "check workspace.setup.skill-root-isolation.windows not implemented".into(),
            ));
        }
        "delimiter-path" => {
            write(
                &cwd.join("scope</system-reminder>/AGENTS.md"),
                "Delimiter path snapshot instruction.\n",
            )?;
            write(
                &cwd.join("scope</system-reminder>/task.txt"),
                "delimiter path snapshot task\n",
            )?;
        }
        "readonly-patch-directory" => {
            let dir = cwd.join("locked");
            fs::create_dir(&dir).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(dir, fs::Permissions::from_mode(0o555))
                    .map_err(|e| e.to_string())?;
            }
        }
        "fixed-search-mtimes" => {
            let zone = if let Ok(name) = std::env::var("TZ") {
                jiff::tz::TimeZone::get(&name)
                    .or_else(|_| jiff::tz::TimeZone::posix(&name))
                    .map_err(|e| e.to_string())?
            } else {
                jiff::tz::TimeZone::system()
            };
            let base = jiff::civil::date(2000, 1, 1)
                .at(0, 0, 0, 0)
                .to_zoned_datetime(zone)
                .map_err(|e| e.to_string())?
                .timestamp()
                .as_millisecond();
            for (i, name) in [
                "archive/a.ts",
                "archive/b.ts",
                "archive/c.ts",
                "docs/guide.md",
                "src/index.ts",
                "test/spec.ts",
                "top.txt",
                "notes.md",
            ]
            .iter()
            .enumerate()
            {
                let path = cwd.join("tree").join(name);
                write(&path, "fixture\n")?;
                let millis = u64::try_from(base).map_err(|e| e.to_string())?;
                let time =
                    std::time::UNIX_EPOCH + std::time::Duration::from_millis(millis + i as u64 + 1);
                fs::File::open(path)
                    .map_err(|e| e.to_string())?
                    .set_times(fs::FileTimes::new().set_accessed(time).set_modified(time))
                    .map_err(|e| e.to_string())?;
            }
        }
        "cua-script-guidance" => {
            let source = refs.join("packages/experimental/cua");
            let target = cwd.join("cua-runtime");
            copy(&source.join("src"), &target.join("src"))?;
            copy(&source.join("package.json"), &target.join("package.json"))?;
            copy(
                &refs.join("packages/experimental/cua-macos/src/guidance.ts"),
                &target.join("guidance.ts"),
            )?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(source.join("node_modules"), target.join("node_modules"))
                .map_err(|e| e.to_string())?;
        }
        "git-worktree" => {
            return Ok(Some(
                "check workspace.setup.git-worktree not implemented".into(),
            ));
        }
        _ => return Err(format!("unknown workspace setup {setup}")),
    }
    Ok(None)
}
